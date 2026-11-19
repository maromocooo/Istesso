//! Which entries of a shown folder are not shown: the files Git ignores, and
//! `.git` itself, unless the owner chose to show everything in that folder.
//!
//! The rules are those of the folder itself: every `.gitignore` from the
//! entry's folder up to the shown folder (the deeper one wins), the shown
//! folder's `.git/info/exclude`, then the user's global ignore file. Patterns
//! are read with the `ignore` crate (ripgrep's), not by hand. Git itself is
//! not run, so this can differ from Git (ADR 0007 lists how).
//!
//! An entry is hidden when it, or any folder on its way from the shown
//! folder, is ignored: Git does not look into an ignored folder, and an
//! entry handed out before a `.gitignore` changed is hidden from then on.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;

/// How long the rules of a folder are used before its `.gitignore` is read
/// again.
const KEEP: Duration = Duration::from_secs(2);
/// The largest ignore file that is read. Larger ones are taken as empty.
pub const MAX_RULES_FILE: usize = 1 << 20;

/// Reads a small regular file below the shown folder, given the folder's
/// names from the shown folder and the file's name, without following links.
pub type ReadSmall<'a> = &'a dyn Fn(&[Vec<u8>], &[u8]) -> Option<Vec<u8>>;

pub struct Rules {
    show_all: bool,
    global: Gitignore,
    /// The rules of each folder, by its names from the shown folder.
    folders: HashMap<Vec<Vec<u8>>, (Instant, Gitignore)>,
    exclude: Option<(Instant, Gitignore)>,
}

/// The rules in one ignore file.
fn parse(contents: &[u8]) -> Gitignore {
    let text = String::from_utf8_lossy(contents);
    // Paths are always given relative to the folder the file is in.
    let mut builder = GitignoreBuilder::new(".");
    for (i, line) in text.lines().enumerate() {
        let line = if i == 0 {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        // A line that is not a pattern is skipped, as Git skips it.
        let _ = builder.add_line(None, line);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

fn relative(names: &[Vec<u8>]) -> PathBuf {
    Path::new(OsStr::from_bytes(&names.join(&b'/'))).to_path_buf()
}

impl Rules {
    /// `global` is the user's global ignore file, read now.
    pub fn new(show_all: bool, global: Option<&Path>) -> Self {
        let global = match (show_all, global) {
            (false, Some(path)) => std::fs::read(path)
                .ok()
                .filter(|contents| contents.len() <= MAX_RULES_FILE)
                .map(|contents| parse(&contents))
                .unwrap_or_else(Gitignore::empty),
            _ => Gitignore::empty(),
        };
        Rules {
            show_all,
            global,
            folders: HashMap::new(),
            exclude: None,
        }
    }

    /// Whether the entry at `names` (from the shown folder) is hidden: it or
    /// a folder on its way is ignored, or it is in or is a `.git`.
    pub fn hides(&mut self, names: &[Vec<u8>], is_dir: bool, read: ReadSmall<'_>) -> bool {
        if self.show_all || names.is_empty() {
            return false;
        }
        if names.iter().any(|name| name == b".git") {
            return true;
        }
        (1..=names.len()).any(|k| self.ignored(&names[..k], k < names.len() || is_dir, read))
    }

    /// Whether the last name of `names` is hidden, when the folders on its
    /// way are known to be shown (an entry of a listed folder).
    pub fn hides_last(&mut self, names: &[Vec<u8>], is_dir: bool, read: ReadSmall<'_>) -> bool {
        if self.show_all || names.is_empty() {
            return false;
        }
        names.last().is_some_and(|name| name == b".git") || self.ignored(names, is_dir, read)
    }

    /// Whether Git's rules ignore `names` itself.
    fn ignored(&mut self, names: &[Vec<u8>], is_dir: bool, read: ReadSmall<'_>) -> bool {
        // The folder the entry is in first, then each folder above it.
        for j in (0..names.len()).rev() {
            let folder = &names[..j];
            let rules = self.folder_rules(folder, read);
            match rules.matched(relative(&names[j..]), is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        let path = relative(names);
        match self.exclude_rules(read).matched(&path, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
        self.global.matched(&path, is_dir).is_ignore()
    }

    fn folder_rules(&mut self, folder: &[Vec<u8>], read: ReadSmall<'_>) -> &Gitignore {
        let fresh = matches!(self.folders.get(folder), Some((at, _)) if at.elapsed() < KEEP);
        if !fresh {
            let rules = read(folder, b".gitignore")
                .map(|contents| parse(&contents))
                .unwrap_or_else(Gitignore::empty);
            self.folders
                .insert(folder.to_vec(), (Instant::now(), rules));
        }
        &self.folders[folder].1
    }

    fn exclude_rules(&mut self, read: ReadSmall<'_>) -> &Gitignore {
        let fresh = matches!(&self.exclude, Some((at, _)) if at.elapsed() < KEEP);
        if !fresh {
            let folder = [b".git".to_vec(), b"info".to_vec()];
            let rules = read(&folder, b"exclude")
                .map(|contents| parse(&contents))
                .unwrap_or_else(Gitignore::empty);
            self.exclude = Some((Instant::now(), rules));
        }
        &self.exclude.as_ref().expect("just set").1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(path: &str) -> Vec<Vec<u8>> {
        path.split('/')
            .map(|name| name.as_bytes().to_vec())
            .collect()
    }

    /// Rules from ignore files given by folder.
    fn files(
        list: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&[Vec<u8>], &[u8]) -> Option<Vec<u8>> {
        move |folder: &[Vec<u8>], name: &[u8]| {
            let mut path = folder.to_vec();
            path.push(name.to_vec());
            let path = String::from_utf8(path.join(&b'/')).unwrap();
            list.iter()
                .find(|(at, _)| *at == path)
                .map(|(_, contents)| contents.as_bytes().to_vec())
        }
    }

    #[test]
    fn what_the_folders_own_rules_ignore_is_hidden() {
        let read = files(&[
            (
                ".gitignore",
                ".env\n/target/\nnode_modules\n*.log\n!keep.log\n",
            ),
            ("web/.gitignore", "dist/\n!.env\n"),
            (".git/info/exclude", "notes.txt\n"),
        ]);
        let mut rules = Rules::new(false, None);
        let mut hides = |path: &str, is_dir: bool| rules.hides(&names(path), is_dir, &read);
        assert!(hides(".env", false));
        assert!(hides("target", true));
        assert!(hides("target/debug/app", false));
        // Anchored to the folder the .gitignore is in.
        assert!(!hides("src/target", true));
        assert!(hides("web/node_modules", true));
        assert!(hides("web/node_modules/x/index.js", false));
        assert!(hides("build.log", false));
        assert!(!hides("keep.log", false));
        assert!(hides("web/dist", true));
        // A deeper .gitignore wins over one above it.
        assert!(!hides("web/.env", false));
        assert!(hides("notes.txt", false));
        assert!(hides(".git", true));
        assert!(hides(".git/config", false));
        assert!(hides("sub/.git", false));
        assert!(!hides("src/main.rs", false));
        assert!(!hides(".gitignore", false));
        assert!(!hides("README.md", false));
        // The shown folder itself is never hidden.
        assert!(!rules.hides(&[], true, &read));
    }

    #[test]
    fn showing_everything_shows_git_and_what_it_ignores() {
        let read = files(&[(".gitignore", ".env\n")]);
        let mut rules = Rules::new(true, None);
        assert!(!rules.hides(&names(".env"), false, &read));
        assert!(!rules.hides(&names(".git/config"), false, &read));
    }

    #[test]
    fn the_last_name_alone_is_checked_for_listed_entries() {
        let read = files(&[(".gitignore", "secret/\n")]);
        let mut rules = Rules::new(false, None);
        assert!(rules.hides_last(&names("secret"), true, &read));
        // Only the last name: the caller has checked the folders on its way.
        assert!(!rules.hides_last(&names("secret/inner"), false, &read));
        assert!(rules.hides(&names("secret/inner"), false, &read));
        assert!(rules.hides_last(&names("a/.git"), true, &read));
    }

    #[test]
    fn a_file_with_a_byte_order_mark_and_windows_lines_is_read() {
        let read = files(&[(".gitignore", "\u{feff}.env\r\n*.tmp\r\n")]);
        let mut rules = Rules::new(false, None);
        assert!(rules.hides(&names(".env"), false, &read));
        assert!(rules.hides(&names("a.tmp"), false, &read));
    }
}
