//! Checks that hold for every adapter, whoever wrote it.
//!
//! What an adapter can name is limited by the compiler: the adapters are a
//! crate of their own without a standard library, whose only dependency is
//! the interface crate (see `istesso-device-adapters`). That is the boundary.
//!
//! The scan of the sources below is **not** a boundary. It is a help for the
//! reviewer: it points at the few words the compiler still allows and that an
//! adapter has no reason to use (`extern crate std;` above all). An adapter
//! is compiled-in code and is trusted like the rest of the program; nothing
//! here is a sandbox.
//!
//! The other check is the replay of the recordings that come with an
//! adapter.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use istesso_devices::{record, registry};
use proc_macro2::{TokenStream, TokenTree};

fn adapters_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../istesso-device-adapters/adapters")
}

/// The directories of the adapters.
fn adapter_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(adapters_dir())
        .expect("the adapters directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn name_of(dir: &Path) -> String {
    dir.file_name()
        .expect("a directory name")
        .to_string_lossy()
        .into_owned()
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Words an adapter has no reason to use, and why a reviewer should look.
const WORDS: &[(&str, &str)] = &[
    (
        "extern",
        "`extern crate std;` would bring the standard library back",
    ),
    (
        "unsafe",
        "an adapter has no unsafe code (the compiler refuses it as well)",
    ),
    ("include", "an adapter is its own source and nothing else"),
    ("include_str", "an adapter reads no files"),
    ("include_bytes", "an adapter reads no files"),
    ("env", "an adapter reads no environment"),
    ("option_env", "an adapter reads no environment"),
    ("path", "`#[path]` would pull in a file from elsewhere"),
    (
        "std",
        "the adapters' crate has no standard library; use `core` and `alloc`",
    ),
];

fn collect(tokens: TokenStream, found: &mut Vec<String>) {
    for token in tokens {
        match token {
            TokenTree::Ident(ident) => {
                let word = ident.to_string();
                if let Some((_, why)) = WORDS.iter().find(|(w, _)| *w == word) {
                    found.push(format!("`{word}`: {why}"));
                }
            }
            TokenTree::Group(group) => collect(group.stream(), found),
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

/// The words of [`WORDS`] that appear in `source` as code. Comments and
/// string literals do not count, spacing does not matter, and the inside of
/// macros is looked at too.
fn words_to_look_at(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    match TokenStream::from_str(source) {
        Ok(tokens) => collect(tokens, &mut found),
        Err(error) => found.push(format!("the source could not be read as Rust: {error}")),
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn the_scan_points_at_the_words_it_is_there_for() {
    let fine = "use alloc::vec;\n// std::fs is not for adapters\nfn f() -> core::fmt::Result { let _ = \"extern crate std\"; Ok(()) }";
    assert!(
        words_to_look_at(fine).is_empty(),
        "comments and strings are not code"
    );
    for bad in [
        "extern crate std;",
        "extern   crate\n std as s;",
        "/* a */ extern /* b */ crate std;",
        "macro_rules! m { () => { extern crate std; } }",
        "use ::std::fs;",
        "use std as s;",
        "#[path = \"../../../../istesso-devices/src/transport.rs\"] mod t;",
        "const KEY: &str = include_str!(\"../../../../secret\");",
        "const HOME: &str = env!(\"HOME\");",
        "fn f() { unsafe { core::hint::unreachable_unchecked() } }",
    ] {
        assert!(
            !words_to_look_at(bad).is_empty(),
            "should be pointed at: {bad}"
        );
    }
}

#[test]
fn no_adapter_uses_a_word_a_reviewer_should_look_at() {
    for dir in adapter_dirs() {
        for file in files_with_extension(&dir, "rs") {
            let source = fs::read_to_string(&file).expect("adapter source");
            let found = words_to_look_at(&source);
            assert!(
                found.is_empty(),
                "{}:\n  {}",
                file.display(),
                found.join("\n  ")
            );
        }
    }
}

#[test]
fn every_directory_is_an_adapter_that_was_built_in() {
    let built: Vec<&str> = registry::entries()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for dir in adapter_dirs() {
        let name = name_of(&dir);
        assert!(
            dir.join("adapter.rs").is_file(),
            "adapters/{name} has no adapter.rs"
        );
        assert!(
            built.contains(&name.as_str()),
            "adapters/{name} was not built in"
        );
    }
    assert_eq!(built.len(), adapter_dirs().len());
}

#[test]
fn every_adapter_comes_with_a_recording_that_replays() {
    for dir in adapter_dirs() {
        let name = name_of(&dir);
        let entry =
            registry::by_name(&name).unwrap_or_else(|| panic!("adapters/{name} was not built in"));
        let recordings = files_with_extension(&dir.join("recordings"), "rec");
        assert!(
            !recordings.is_empty(),
            "adapters/{name} has no recording under recordings/"
        );
        for file in recordings {
            let text = fs::read_to_string(&file).expect("a recording");
            assert_eq!(
                record::adapter_of(&text),
                Some(name.as_str()),
                "{}: the recording must say `adapter {name}`",
                file.display()
            );
            if let Err(error) = record::replay(entry, &text) {
                panic!("{} does not replay: {error}", file.display());
            }
        }
    }
}
