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
//!
//! Names are compared the way the folder's file system compares them
//! ([`Folding`]): on a volume that does not tell upper from lower case (the
//! Mac's default), `.ENV` is the file `.env` and is hidden by a rule for
//! `.env`, as Git does there (`core.ignorecase`); on the Mac, names and rules
//! are compared in their precomposed Unicode form (NFC), as Git does there
//! (`core.precomposeunicode`). `.git` is hidden in any case. The caller only
//! asks about names as they are stored on disk ([`super::export`]), so a
//! device cannot pick a spelling that the file system finds and these rules
//! do not.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use unicode_normalization::UnicodeNormalization;

/// How long the rules of a folder are used before its `.gitignore` is read
/// again.
const KEEP: Duration = Duration::from_secs(2);
/// The largest ignore file that is read. Larger ones are taken as empty.
pub const MAX_RULES_FILE: usize = 1 << 20;

/// Reads a small regular file below the shown folder, given the folder's
/// names from the shown folder and the file's name, without following links.
pub type ReadSmall<'a> = &'a dyn Fn(&[Vec<u8>], &[u8]) -> Option<Vec<u8>>;

/// How the shown folder's file system compares names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Folding {
    /// Upper and lower case are the same name.
    pub case: bool,
    /// A name and its other Unicode form (NFC, NFD) are the same name.
    pub unicode: bool,
}

impl Folding {
    /// A name as it is compared with the rules.
    fn name(&self, name: &[u8]) -> Vec<u8> {
        match std::str::from_utf8(name) {
            Ok(text) if self.unicode => text.nfc().collect::<String>().into_bytes(),
            _ => name.to_vec(),
        }
    }

    /// Whether `name` is `.git` on this file system. Any spelling counts:
    /// hiding a folder named `.GIT` on a volume that tells case apart costs
    /// nothing.
    fn is_git(&self, name: &[u8]) -> bool {
        name.eq_ignore_ascii_case(b".git")
    }
}

pub struct Rules {
    show_all: bool,
    folding: Folding,
    global: Gitignore,
    /// The rules of each folder, by its names from the shown folder.
    folders: HashMap<Vec<Vec<u8>>, (Instant, Gitignore)>,
    exclude: Option<(Instant, Gitignore)>,
}

/// The rules in one ignore file.
fn parse(contents: &[u8], folding: Folding) -> Gitignore {
    let text = String::from_utf8_lossy(contents);
    let text: String = if folding.unicode {
        text.nfc().collect()
    } else {
        text.into_owned()
    };
    // Paths are always given relative to the folder the file is in.
    let mut builder = GitignoreBuilder::new(".");
    let _ = builder.case_insensitive(folding.case);
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

fn relative(names: &[Vec<u8>], folding: Folding) -> PathBuf {
    let names: Vec<Vec<u8>> = names.iter().map(|name| folding.name(name)).collect();
    Path::new(OsStr::from_bytes(&names.join(&b'/'))).to_path_buf()
}

impl Rules {
    /// `global` is the user's global ignore file, read now. `folding` is how
    /// the shown folder's file system compares names.
    pub fn new(show_all: bool, global: Option<&Path>, folding: Folding) -> Self {
        let global = match (show_all, global) {
            (false, Some(path)) => std::fs::read(path)
                .ok()
                .filter(|contents| contents.len() <= MAX_RULES_FILE)
                .map(|contents| parse(&contents, folding))
                .unwrap_or_else(Gitignore::empty),
            _ => Gitignore::empty(),
        };
        Rules {
            show_all,
            folding,
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
        if names.iter().any(|name| self.folding.is_git(name)) {
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
        names.last().is_some_and(|name| self.folding.is_git(name))
            || self.ignored(names, is_dir, read)
    }

    /// Whether Git's rules ignore `names` itself.
    fn ignored(&mut self, names: &[Vec<u8>], is_dir: bool, read: ReadSmall<'_>) -> bool {
        let folding = self.folding;
        // The folder the entry is in first, then each folder above it.
        for j in (0..names.len()).rev() {
            let folder = &names[..j];
            let rules = self.folder_rules(folder, read);
            match rules.matched(relative(&names[j..], folding), is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        let path = relative(names, folding);
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
            let folding = self.folding;
            let rules = read(folder, b".gitignore")
                .map(|contents| parse(&contents, folding))
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
            let folding = self.folding;
            let rules = read(&folder, b"exclude")
                .map(|contents| parse(&contents, folding))
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
        let mut rules = Rules::new(false, None, Folding::default());
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
        let mut rules = Rules::new(true, None, Folding::default());
        assert!(!rules.hides(&names(".env"), false, &read));
        assert!(!rules.hides(&names(".git/config"), false, &read));
    }

    #[test]
    fn the_last_name_alone_is_checked_for_listed_entries() {
        let read = files(&[(".gitignore", "secret/\n")]);
        let mut rules = Rules::new(false, None, Folding::default());
        assert!(rules.hides_last(&names("secret"), true, &read));
        // Only the last name: the caller has checked the folders on its way.
        assert!(!rules.hides_last(&names("secret/inner"), false, &read));
        assert!(rules.hides(&names("secret/inner"), false, &read));
        assert!(rules.hides_last(&names("a/.git"), true, &read));
    }

    #[test]
    fn on_a_volume_that_folds_names_rules_fold_them_too() {
        let read = files(&[(".gitignore", ".env\ncaf\u{e9}.txt\nSecrets/\n")]);
        let folds = Folding {
            case: true,
            unicode: true,
        };
        let mut rules = Rules::new(false, None, folds);
        let mut hides = |path: &str, is_dir: bool| rules.hides(&names(path), is_dir, &read);
        assert!(hides(".ENV", false));
        assert!(hides(".Env", false));
        assert!(hides("secrets", true));
        assert!(hides("SECRETS/key", false));
        // The same name decomposed (NFD) is the same name.
        assert!(hides("cafe\u{301}.txt", false));
        assert!(hides("caf\u{e9}.txt", false));
        assert!(hides(".GIT", true));
        assert!(hides(".Git/config", false));
        assert!(!hides("README.md", false));

        // A volume that tells them apart: other spellings are other files,
        // except that .git is hidden in any spelling.
        let mut exact = Rules::new(false, None, Folding::default());
        assert!(!exact.hides(&names(".ENV"), false, &read));
        assert!(!exact.hides(&names("cafe\u{301}.txt"), false, &read));
        assert!(exact.hides(&names(".GIT"), true, &read));
    }

    #[test]
    fn a_file_with_a_byte_order_mark_and_windows_lines_is_read() {
        let read = files(&[(".gitignore", "\u{feff}.env\r\n*.tmp\r\n")]);
        let mut rules = Rules::new(false, None, Folding::default());
        assert!(rules.hides(&names(".env"), false, &read));
        assert!(rules.hides(&names("a.tmp"), false, &read));
    }
}
