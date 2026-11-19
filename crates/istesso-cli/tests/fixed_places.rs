//! A program of the same user starts `istesso` with the arguments and the
//! environment it likes. The places that decide which key, which paired
//! devices and which sockets are used do not follow them (ADR 0021).
//! Nothing here reads or writes this account's real files: every command is
//! refused before it would.

use std::process::Command;

#[cfg(unix)]
#[test]
fn the_relay_does_not_listen_where_it_is_told() {
    let elsewhere =
        std::env::temp_dir().join(format!("istesso-fixed-places-{}", std::process::id()));
    std::fs::create_dir_all(&elsewhere).unwrap();
    let socket = elsewhere.join("in.sock");
    let result = Command::new(env!("CARGO_BIN_EXE_istesso"))
        .args(["input", "relay", "--to", "nobody", "--socket"])
        .arg(&socket)
        // Pointing the program's places elsewhere through the environment
        // changes nothing either.
        .env("HOME", &elsewhere)
        .env("XDG_CACHE_HOME", &elsewhere)
        .env("XDG_RUNTIME_DIR", &elsewhere)
        .env("ISTESSO_CONFIG_DIR", &elsewhere)
        .env("ISTESSO_CACHE_DIR", &elsewhere)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("is not a place of Istesso's"), "{stderr}");
    assert!(!socket.exists());
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    std::fs::remove_dir_all(&elsewhere).unwrap();
}
