//! Whether a group can hold anyone but the user who runs the command.
//!
//! A task file, or a folder leading to it, that its group can write is
//! trusted only when the group contains nobody but this user (the model
//! Debian's and Ubuntu's OpenSSH use for group-writable key files). Ubuntu's
//! default umask (002) makes folders 775 and files 664 with the user's own
//! private group, which holds no one else.
//!
//! The rule, for the item's group:
//! 1. every account whose primary group it is has this user's uid (compared
//!    by uid, never by name);
//! 2. every member the group lists is this user's name;
//! 3. at least one of the two finds this user (an empty group that nobody
//!    has as primary group is refused).
//!
//! On Linux the answer comes from the system's own lookup, which goes
//! through every configured source: `getent passwd` (all accounts) and
//! `getent group <gid>`, run without a shell. If either cannot be run or
//! fails, or the account list is inconsistent, the group counts as shared.
//! On macOS and other systems a group-writable item is not trusted at all:
//! the default umask there is 022 and the usual primary group is shared.
//!
//! Not seen: a group password set by the administrator (with which others
//! could join the group through `newgrp`), and accounts of a directory
//! service that does not list all of them.

use std::sync::OnceLock;

/// Where the answers come from.
#[derive(Debug)]
enum Source {
    /// The system's own lookup (`getent`), asked when first needed.
    System {
        passwd: OnceLock<Result<String, String>>,
    },
    /// Given outputs, for tests: `getent passwd`, and the lines that
    /// `getent group <gid>` would print for any gid.
    Given {
        passwd: Result<String, String>,
        groups: Result<String, String>,
    },
}

/// The account lookup.
#[derive(Debug)]
pub(crate) struct AccountDb {
    source: Source,
}

impl Default for AccountDb {
    /// A lookup that always fails: every group counts as shared.
    fn default() -> AccountDb {
        AccountDb::given(None, None)
    }
}

impl AccountDb {
    /// The system's own lookup.
    pub(crate) fn system() -> AccountDb {
        AccountDb {
            source: Source::System {
                passwd: OnceLock::new(),
            },
        }
    }

    /// Given outputs (`None`: the lookup failed).
    pub(crate) fn given(passwd: Option<String>, groups: Option<String>) -> AccountDb {
        let failed = || Err("the account lookup failed".to_owned());
        AccountDb {
            source: Source::Given {
                passwd: passwd.map(Ok).unwrap_or_else(failed),
                groups: groups.map(Ok).unwrap_or_else(failed),
            },
        }
    }

    fn system_elsewhere(&self) -> bool {
        matches!(self.source, Source::System { .. }) && !cfg!(target_os = "linux")
    }

    fn passwd(&self) -> Result<String, String> {
        match &self.source {
            Source::System { passwd } => passwd.get_or_init(|| getent(&["passwd"])).clone(),
            Source::Given { passwd, .. } => passwd.clone(),
        }
    }

    fn group(&self, gid: u32) -> Result<String, String> {
        match &self.source {
            Source::System { .. } => getent(&["group", &gid.to_string()]),
            Source::Given { groups, .. } => groups.clone().map(|text| {
                text.lines()
                    .filter(|l| l.split(':').nth(2).and_then(|g| g.parse().ok()) == Some(gid))
                    .map(|l| format!("{l}\n"))
                    .collect()
            }),
        }
    }

    /// `Ok` when the group `gid` contains nobody but the user `uid`; else why
    /// it may hold someone else, or why that cannot be told.
    pub(crate) fn is_private(&self, gid: u32, uid: u32) -> Result<(), String> {
        if self.system_elsewhere() {
            return Err(
                "on this system a group-writable item is not trusted (the default umask is 022 and the usual primary group is shared)"
                    .into(),
            );
        }
        let passwd = self.passwd()?;
        let group = self.group(gid)?;
        group_is_private(gid, uid, &passwd, &group)
    }

    /// The group's name for messages; the number when it cannot be found.
    pub(crate) fn group_name(&self, gid: u32) -> String {
        if self.system_elsewhere() {
            return gid.to_string();
        }
        self.group(gid)
            .ok()
            .and_then(|text| {
                text.lines()
                    .next()
                    .and_then(|l| l.split(':').next())
                    .map(str::to_owned)
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| gid.to_string())
    }
}

/// Runs `/usr/bin/getent` with `args`: no shell, `LC_ALL=C`, standard input
/// closed. Any failure, including "not found" (exit code 2), is an error.
fn getent(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("/usr/bin/getent")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            format!(
                "the account lookup (`getent {}`) cannot be run: {e}",
                args[0]
            )
        })?;
    match output.status.code() {
        Some(0) => String::from_utf8(output.stdout)
            .map_err(|_| format!("`getent {}` printed text that is not UTF-8", args[0])),
        Some(2) if args[0] == "group" => Err("the group is not found by the account lookup".into()),
        _ => Err(format!("the account lookup (`getent {}`) failed", args[0])),
    }
}

/// The accounts of `getent passwd`: name, uid, primary gid.
fn accounts(passwd: &str) -> Result<Vec<(&str, u32, u32)>, String> {
    let unreadable = || "the account list has a line that cannot be read".to_owned();
    passwd
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() < 4 || fields[0].is_empty() {
                return Err(unreadable());
            }
            match (fields[2].parse(), fields[3].parse()) {
                (Ok(uid), Ok(gid)) => Ok((fields[0], uid, gid)),
                _ => Err(unreadable()),
            }
        })
        .collect()
}

/// The rule (see the module's description), on the output of
/// `getent passwd` and of `getent group <gid>`.
pub(crate) fn group_is_private(
    gid: u32,
    uid: u32,
    passwd: &str,
    group: &str,
) -> Result<(), String> {
    let accounts = accounts(passwd)?;

    // Who we are, consistently.
    let mut ours: Vec<&str> = accounts
        .iter()
        .filter(|(_, u, _)| *u == uid)
        .map(|(n, _, _)| *n)
        .collect();
    ours.sort_unstable();
    ours.dedup();
    let name = match ours.as_slice() {
        [name] => *name,
        [] => return Err("this user is not found by the account lookup".into()),
        _ => {
            return Err("the account list is inconsistent: this user's id has several names".into())
        }
    };
    if accounts.iter().any(|(n, u, _)| *n == name && *u != uid) {
        return Err(format!(
            "the account list is inconsistent: the name `{name}` has several ids"
        ));
    }

    // 1. Every account with this primary group is us, by uid.
    let mut found = false;
    for (other, other_uid, primary) in &accounts {
        if *primary != gid {
            continue;
        }
        if *other_uid != uid {
            return Err(format!(
                "it is the primary group of the account `{other}` (id {other_uid})"
            ));
        }
        found = true;
    }

    // 2. Every listed member is us, by name.
    let lines: Vec<&str> = group.lines().filter(|l| !l.trim().is_empty()).collect();
    let [line] = lines.as_slice() else {
        return Err(if lines.is_empty() {
            "the group is not found by the account lookup".into()
        } else {
            "the account lookup gave several lines for the group".into()
        });
    };
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 4 || fields[2].parse() != Ok(gid) {
        return Err("the group's line cannot be read".into());
    }
    for member in fields[3]
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        if member != name {
            return Err(format!("the group lists `{member}` as a member"));
        }
        found = true;
    }

    // 3. It holds us.
    if !found {
        return Err("the group holds no one: it is not this user's own group".into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/sh\nme:x:1000:1000:Me:/nonexistent:/bin/sh\n";

    /// A lookup in which `me` (the given uid) has the private group `me`
    /// (the given gid). Other accounts are left out when they would share
    /// an id (in a temporary folder on macOS, files get the group of id 0).
    pub(crate) fn private_db(uid: u32, gid: u32) -> AccountDb {
        let mut passwd = String::new();
        let mut groups = String::new();
        if uid != 0 && gid != 0 {
            passwd.push_str("root:x:0:0:root:/root:/bin/sh\n");
            groups.push_str("root:x:0:\n");
        }
        if gid != 4 {
            groups.push_str("adm:x:4:syslog\n");
        }
        passwd.push_str(&format!("me:x:{uid}:{gid}:Me:/nonexistent:/bin/sh\n"));
        groups.push_str(&format!("me:x:{gid}:\n"));
        AccountDb::given(Some(passwd), Some(groups))
    }

    #[test]
    fn the_user_s_private_group_is_private() {
        assert_eq!(group_is_private(1000, 1000, PASSWD, "me:x:1000:\n"), Ok(()));
        // Listing the user as a member as well is still private.
        assert_eq!(
            group_is_private(1000, 1000, PASSWD, "me:x:1000:me\n"),
            Ok(())
        );
        // A group that is nobody's primary group but lists only us.
        assert_eq!(
            group_is_private(1001, 1000, PASSWD, "solo:x:1001:me\n"),
            Ok(())
        );
        assert_eq!(private_db(1000, 1000).is_private(1000, 1000), Ok(()));
        assert_eq!(private_db(1000, 1000).group_name(4), "adm");
        assert_eq!(private_db(1000, 1000).group_name(77), "77");
    }

    #[test]
    fn a_second_account_with_the_same_primary_group_is_refused() {
        let passwd = format!("{PASSWD}guest:x:1001:1000::/nonexistent:/bin/sh\n");
        let why = group_is_private(1000, 1000, &passwd, "me:x:1000:\n").unwrap_err();
        assert!(why.contains("`guest`"), "{why}");
        // Also when it carries our name with another id.
        let passwd = format!("{PASSWD}me:x:1001:1000::/nonexistent:/bin/sh\n");
        let why = group_is_private(1000, 1000, &passwd, "me:x:1000:\n").unwrap_err();
        assert!(why.contains("inconsistent"), "{why}");
        // Or our id under another name.
        let passwd = format!("{PASSWD}alias:x:1000:1000::/nonexistent:/bin/sh\n");
        let why = group_is_private(1000, 1000, &passwd, "me:x:1000:\n").unwrap_err();
        assert!(why.contains("inconsistent"), "{why}");
    }

    #[test]
    fn a_listed_member_that_is_not_us_is_refused() {
        let why = group_is_private(1000, 1000, PASSWD, "me:x:1000:me,guest\n").unwrap_err();
        assert!(why.contains("`guest`"), "{why}");
    }

    #[test]
    fn a_group_that_is_not_found_or_holds_no_one_is_refused() {
        let why = group_is_private(1000, 1000, PASSWD, "").unwrap_err();
        assert!(why.contains("not found"), "{why}");
        // `staff`: nobody's primary group here, no members, so not ours.
        let why = group_is_private(20, 1000, PASSWD, "staff:x:20:\n").unwrap_err();
        assert!(why.contains("holds no one"), "{why}");
        assert!(private_db(1000, 1000).is_private(20, 1000).is_err());
    }

    #[test]
    fn a_failed_lookup_is_refused() {
        let why = AccountDb::default().is_private(1000, 1000).unwrap_err();
        assert!(why.contains("lookup failed"), "{why}");
        let db = AccountDb::given(Some(PASSWD.into()), None);
        assert!(db.is_private(1000, 1000).is_err());
        // Not listed, or a line that cannot be read.
        assert!(group_is_private(1000, 2000, PASSWD, "me:x:1000:\n")
            .unwrap_err()
            .contains("not found"));
        assert!(group_is_private(1000, 1000, "me:x:abc:1000\n", "me:x:1000:\n").is_err());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn elsewhere_than_linux_group_write_is_not_trusted() {
        let why = AccountDb::system().is_private(20, 501).unwrap_err();
        assert!(why.contains("umask is 022"), "{why}");
    }
}
