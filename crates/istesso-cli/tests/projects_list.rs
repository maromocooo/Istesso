use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Register(PathBuf);

impl Register {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("project-list-{tag}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let register = Self(root);
        let paths = istesso_agent::Paths::at(register.0.join("config"));
        istesso_agent::projects::add(&paths, "good", &register.0).unwrap();
        register
    }

    fn check_list(&self) {
        for json in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_istesso"));
            command
                .env("ISTESSO_CONFIG_DIR", self.0.join("config"))
                .args(["projects", "list"]);
            if json {
                command.arg("--json");
            }
            let output = command.output().unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stderr).unwrap().trim_end(),
                "1 project skipped"
            );
            if json {
                let projects: Vec<istesso_proto::Project> =
                    serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(projects.len(), 1);
                assert_eq!(projects[0].name, "good");
            } else {
                assert_eq!(
                    String::from_utf8(output.stdout).unwrap(),
                    "PROJECT\tKINDS\ngood\tunknown\n"
                );
            }
        }
    }
}

impl Drop for Register {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn damaged_registration_is_counted_and_healthy_rows_are_printed() {
    let register = Register::new("damaged");
    fs::write(register.0.join("config/projects/project-broken.json"), b"{").unwrap();
    register.check_list();
}

#[cfg(unix)]
#[test]
fn unreadable_folder_is_counted_and_healthy_rows_are_printed() {
    use std::os::unix::fs::PermissionsExt;

    let register = Register::new("unreadable");
    let paths = istesso_agent::Paths::at(register.0.join("config"));
    let folder = register.0.join("folder");
    fs::create_dir(&folder).unwrap();
    istesso_agent::projects::add(&paths, "unreadable", &folder).unwrap();
    let original = fs::metadata(&folder).unwrap().permissions();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&folder).is_ok() {
        // Privileged runners bypass mode 000. Use a missing folder to
        // exercise the same read-error path there.
        fs::set_permissions(&folder, original.clone()).unwrap();
        fs::remove_dir(&folder).unwrap();
    } else {
        register.check_list();
        fs::set_permissions(&folder, original).unwrap();
        return;
    }
    register.check_list();
}
