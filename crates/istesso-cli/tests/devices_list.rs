//! Exercise both CLI formats without inspecting the machine running the test.

#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn local_list_from_captured_os_output() {
    struct Tools(PathBuf);
    impl Drop for Tools {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    let dir = std::env::temp_dir().join(format!("device-tools-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let tools = Tools(dir);
    let install = |name: &str, script: &str| {
        let path = tools.0.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    };

    install(
        "system_profiler",
        r#"#!/bin/sh
[ "$*" = "SPAudioDataType -json" ] || exit 1
/bin/cat "$FIXTURES/mac-system_profiler.json"
echo "Example Diagnostic" >&2
"#,
    );
    install(
        "ioreg",
        r#"#!/bin/sh
case "$*" in
  "-a -l -r -c IOUSBHostDevice -d 1") file=mac-ioreg-usb.plist ;;
  "-a -l -r -c IOUSBHostInterface -d 1") file=mac-ioreg-usb-interfaces.plist ;;
  "-a -l -r -c IOHIDDevice -d 1") file=mac-ioreg-hid.plist ;;
  "-a -l -r -n isp -d 2") file=mac-ioreg-isp.plist ;;
  *) exit 1 ;;
esac
/bin/cat "$FIXTURES/$file"
"#,
    );
    let run = |options: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_istesso"))
            .args(["devices", "list"])
            .args(options)
            .env("PATH", &tools.0)
            .env(
                "FIXTURES",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../istesso-devices/tests/fixtures/real-shape"
                ),
            )
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stderr.is_empty());
        String::from_utf8(result.stdout).unwrap()
    };
    let text = run(&[]);
    let json = run(&["--json"]);
    let devices: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert_eq!(devices.len(), text.lines().count());
    for (model, kind, usage, row) in [
        (
            "Camera",
            "camera",
            "local_apps",
            "Camera (built-in) | camera (use with apps on this PC)",
        ),
        (
            "Microphone",
            "audio",
            "carry_audio",
            "Microphone (built-in) | audio (carry audio)",
        ),
    ] {
        assert!(text.lines().any(|line| line == row));
        assert!(devices.contains(&serde_json::json!({
            "model":model, "usb":null, "name_hidden":false, "port":null, "built_in":true,
            "kinds":[{"kind":kind,"usage":usage}]
        })));
    }
    assert!(text.contains("USB fffd:fefe (name hidden) at 128-1.1 (unknown)"));
    for private in [
        "Example Café Camera",
        "Example Toy's Camera", // check-private: ok (sanitized camera label)
        "Example Device",
        "SERIAL-", // check-private: ok (fixture placeholder prefix)
    ] {
        assert!(!text.contains(private) && !json.contains(private));
    }
}
