//! Whether a group can hold anyone but the user who runs the command.
//!
//! A task file, or a folder leading to it, that its group can write is
//! trusted only when the group contains nobody but this user (the model
//! Debian's and Ubuntu's OpenSSH use for group-writable key files). Ubuntu's
//! default umask (002) makes folders 775 and files 664 with the user's own
//! private group, which holds no one else.
//!
//! The rule, for the item's group; all must hold:
//! 1. Sources: every `passwd`, `group` and `initgroups` line of
//!    `/etc/nsswitch.conf` names only `files` or `systemd`, and no
//!    database is named twice. Any other source (a directory service) may
//!    not list all its accounts when enumerated.
//! 2. Accounts, from `getent passwd` (enumerated): every account whose
//!    primary group it is has this user's uid (compared by uid, never by
//!    name); this uid has one name only, and that name one uid only.
//! 3. Groups, from `getent group` (enumerated, no key): exactly one entry has
//!    the gid, and every member it lists is this user's name.
//! 4. At least one of 2 and 3 finds this user.
//!
//! Anything that cannot be established refuses. `getent` is run without a
//! shell, with `LC_ALL=C` and standard input closed. On macOS and other
//! systems a group-writable item is not trusted at all: the default umask
//! there is 022 and the usual primary group is shared.
//!
//! Not seen: a group password set by the administrator, with which others
//! could join the group (the file that holds it cannot be read without root).

use std::sync::OnceLock;

/// The databases' text: `/etc/nsswitch.conf`, `getent passwd`, `getent group`.
#[derive(Debug, Clone)]
struct Texts {
    nsswitch: Result<String, String>,
    passwd: Result<String, String>,
    groups: Result<String, String>,
}

/// The account lookup.
#[derive(Debug)]
pub(crate) struct AccountDb {
    /// `None` for the system's own, read when first needed.
    given: Option<Texts>,
    system: OnceLock<Texts>,
}

impl Default for AccountDb {
    /// A lookup that always fails: every group counts as shared.
    fn default() -> AccountDb {
        AccountDb::given(None, None, None)
    }
}

impl AccountDb {
    /// The system's own sources.
    pub(crate) fn system() -> AccountDb {
        AccountDb {
            given: None,
            system: OnceLock::new(),
        }
    }

    /// Given texts, for tests (`None`: it could not be read or run).
    pub(crate) fn given(
        nsswitch: Option<String>,
        passwd: Option<String>,
        groups: Option<String>,
    ) -> AccountDb {
        let failed = |what: &str| Err(format!("{what} could not be read"));
        AccountDb {
            given: Some(Texts {
                nsswitch: nsswitch
                    .map(Ok)
                    .unwrap_or_else(|| failed("/etc/nsswitch.conf")),
                passwd: passwd
                    .map(Ok)
                    .unwrap_or_else(|| failed("the account lookup (`getent passwd`)")),
                groups: groups
                    .map(Ok)
                    .unwrap_or_else(|| failed("the group lookup (`getent group`)")),
            }),
            system: OnceLock::new(),
        }
    }

    fn system_elsewhere(&self) -> bool {
        self.given.is_none() && !cfg!(target_os = "linux")
    }

    fn texts(&self) -> &Texts {
        if let Some(given) = &self.given {
            return given;
        }
        self.system.get_or_init(|| Texts {
            nsswitch: std::fs::read_to_string("/etc/nsswitch.conf")
                .map_err(|e| format!("/etc/nsswitch.conf could not be read: {e}")),
            passwd: getent("passwd"),
            groups: getent("group"),
        })
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
        let texts = self.texts();
        let nsswitch = texts.nsswitch.as_deref().map_err(Clone::clone)?;
        let passwd = texts.passwd.as_deref().map_err(Clone::clone)?;
        let groups = texts.groups.as_deref().map_err(Clone::clone)?;
        group_is_private(gid, uid, nsswitch, passwd, groups)
    }

    /// The group's name for messages; the number when it cannot be found.
    pub(crate) fn group_name(&self, gid: u32) -> String {
        if self.system_elsewhere() {
            return gid.to_string();
        }
        self.texts()
            .groups
            .as_deref()
            .ok()
            .and_then(|text| {
                text.lines()
                    .map(|l| l.split(':').collect::<Vec<_>>())
                    .find(|f| f.len() >= 3 && f[2].parse() == Ok(gid))
                    .map(|f| f[0].to_owned())
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| gid.to_string())
    }
}

/// Runs `/usr/bin/getent <database>` (the whole database): no shell,
/// `LC_ALL=C`, standard input closed. Any failure is an error.
fn getent(database: &str) -> Result<String, String> {
    let output = std::process::Command::new("/usr/bin/getent")
        .arg(database)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("the account lookup (`getent {database}`) cannot be run: {e}"))?;
    if !output.status.success() {
        return Err(format!("the account lookup (`getent {database}`) failed"));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("`getent {database}` printed text that is not UTF-8"))
}

/// The sources `nsswitch.conf` may name for accounts and groups. Not
/// `compat`: it pulls in entries from elsewhere through `+`/`-` lines.
const LOCAL_SOURCES: [&str; 2] = ["files", "systemd"];

/// Rule 1: accounts and groups come from local sources only. Read strictly,
/// as the system's own reader does: blank lines and comments are skipped;
/// the database name starts after leading blanks, ends at a blank or a
/// colon and is compared without regard to case; a line that cannot be read
/// as `name: sources` refuses. Bracketed actions (`[NOTFOUND=return]`) are
/// not sources.
fn local_sources_only(nsswitch: &str) -> Result<(), String> {
    let mut seen: Vec<String> = Vec::new();
    for (number, line) in nsswitch.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default();
        if line.trim().is_empty() {
            continue;
        }
        let unreadable = || format!("line {} of /etc/nsswitch.conf cannot be read", number + 1);
        let rest = line.trim_start();
        let end = rest
            .find(|c: char| c == ':' || c.is_whitespace())
            .unwrap_or(rest.len());
        let (name, after) = rest.split_at(end);
        let Some(sources) = after.trim_start().strip_prefix(':') else {
            return Err(unreadable());
        };
        if name.is_empty() {
            return Err(unreadable());
        }
        let database = name.to_ascii_lowercase();
        if !["passwd", "group", "initgroups"].contains(&database.as_str()) {
            continue;
        }
        if seen.contains(&database) {
            return Err(format!(
                "/etc/nsswitch.conf names `{database}` more than once"
            ));
        }
        // Drop the `[STATUS=action]` items, which name no source.
        let mut plain = String::new();
        let mut depth = 0i32;
        for c in sources.chars() {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ if depth == 0 => plain.push(c),
                _ => {}
            }
            if depth < 0 {
                return Err(unreadable());
            }
        }
        if depth != 0 {
            return Err(unreadable());
        }
        for source in plain.split_whitespace() {
            if !LOCAL_SOURCES.contains(&source) {
                return Err(format!(
                    "accounts may come from `{source}` (/etc/nsswitch.conf, `{database}`), which may not list all of them or may pull in entries from elsewhere"
                ));
            }
        }
        seen.push(database);
    }
    for database in ["passwd", "group"] {
        if !seen.iter().any(|d| d == database) {
            return Err(format!(
                "/etc/nsswitch.conf does not say where `{database}` comes from"
            ));
        }
    }
    Ok(())
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

/// The rule (see the module's description), on the text of
/// `/etc/nsswitch.conf`, `getent passwd` and `getent group` (no key).
pub(crate) fn group_is_private(
    gid: u32,
    uid: u32,
    nsswitch: &str,
    passwd: &str,
    groups: &str,
) -> Result<(), String> {
    // 1. Local sources only.
    local_sources_only(nsswitch)?;

    // Who we are, consistently.
    let accounts = accounts(passwd)?;
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

    // 2. Every account with this primary group is us, by uid.
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

    // 3. Exactly one group has the gid, and every member it lists is us.
    let entries: Vec<Vec<&str>> = groups
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(':').collect::<Vec<_>>())
        .collect();
    if entries
        .iter()
        .any(|f| f.len() < 4 || f[2].parse::<u32>().is_err())
    {
        return Err("the group list has a line that cannot be read".into());
    }
    let matching: Vec<&Vec<&str>> = entries.iter().filter(|f| f[2].parse() == Ok(gid)).collect();
    let [fields] = matching.as_slice() else {
        return Err(if matching.is_empty() {
            "the group is not found by the account lookup".into()
        } else {
            format!("{} groups have the id {gid}", matching.len())
        });
    };
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

    // 4. It holds us.
    if !found {
        return Err("the group holds no one: it is not this user's own group".into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Ubuntu's default.
    pub(crate) const NSSWITCH: &str = "passwd:         files systemd\ngroup:          files systemd\nshadow:         files systemd\nhosts:          files mdns4_minimal [NOTFOUND=return] dns\n";
    const PASSWD: &str = "root:x:0:0:root:/root:/bin/sh\nme:x:1000:1000:Me:/nonexistent:/bin/sh\n";
    const GROUPS: &str = "root:x:0:\nadm:x:4:syslog\nme:x:1000:\n";

    fn private(gid: u32, uid: u32, passwd: &str, groups: &str) -> Result<(), String> {
        group_is_private(gid, uid, NSSWITCH, passwd, groups)
    }

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
        AccountDb::given(Some(NSSWITCH.into()), Some(passwd), Some(groups))
    }

    #[test]
    fn the_user_s_private_group_is_private() {
        // Ubuntu's default sources, `files systemd`.
        assert_eq!(private(1000, 1000, PASSWD, GROUPS), Ok(()));
        // Listing the user as a member as well is still private.
        assert_eq!(private(1000, 1000, PASSWD, "me:x:1000:me\n"), Ok(()));
        // A group that is nobody's primary group but lists only us.
        assert_eq!(
            private(1001, 1000, PASSWD, "me:x:1000:\nsolo:x:1001:me\n"),
            Ok(())
        );
        assert_eq!(private_db(1000, 1000).is_private(1000, 1000), Ok(()));
        assert_eq!(private_db(1000, 1000).group_name(4), "adm");
        assert_eq!(private_db(1000, 1000).group_name(77), "77");
    }

    #[test]
    fn a_second_account_with_the_same_primary_group_is_refused() {
        let passwd = format!("{PASSWD}guest:x:1001:1000::/nonexistent:/bin/sh\n");
        let why = private(1000, 1000, &passwd, GROUPS).unwrap_err();
        assert!(why.contains("`guest`"), "{why}");
        // Also when it carries our name with another id.
        let passwd = format!("{PASSWD}me:x:1001:1000::/nonexistent:/bin/sh\n");
        let why = private(1000, 1000, &passwd, GROUPS).unwrap_err();
        assert!(why.contains("inconsistent"), "{why}");
        // Or our id under another name.
        let passwd = format!("{PASSWD}alias:x:1000:1000::/nonexistent:/bin/sh\n");
        let why = private(1000, 1000, &passwd, GROUPS).unwrap_err();
        assert!(why.contains("inconsistent"), "{why}");
    }

    #[test]
    fn a_listed_member_that_is_not_us_is_refused() {
        let why = private(1000, 1000, PASSWD, "me:x:1000:me,guest\n").unwrap_err();
        assert!(why.contains("`guest`"), "{why}");
    }

    #[test]
    fn two_groups_with_the_same_id_are_refused() {
        let groups = format!("{GROUPS}others:x:1000:guest\n");
        let why = private(1000, 1000, PASSWD, &groups).unwrap_err();
        assert!(why.contains("2 groups have the id 1000"), "{why}");
        let groups = format!("{GROUPS}copy:x:1000:\n");
        assert!(private(1000, 1000, PASSWD, &groups).is_err());
    }

    #[test]
    fn a_group_that_is_not_found_or_holds_no_one_is_refused() {
        let why = private(1000, 1000, PASSWD, "root:x:0:\n").unwrap_err();
        assert!(why.contains("not found"), "{why}");
        // `staff`: nobody's primary group here, no members, so not ours.
        let why = private(20, 1000, PASSWD, "staff:x:20:\n").unwrap_err();
        assert!(why.contains("holds no one"), "{why}");
        assert!(private_db(1000, 1000).is_private(20, 1000).is_err());
    }

    #[test]
    fn sources_other_than_local_files_are_refused() {
        let check = |nsswitch: &str| group_is_private(1000, 1000, nsswitch, PASSWD, GROUPS);
        assert_eq!(check("passwd: files\ngroup: files\n"), Ok(()));
        // A bracketed action is not a source.
        assert_eq!(
            check("passwd: files [NOTFOUND=return] systemd\ngroup: files\n"),
            Ok(())
        );
        // `compat` pulls in entries from elsewhere.
        let why = check("passwd: compat\ngroup: files\n").unwrap_err();
        assert!(why.contains("`compat`"), "{why}");
        // Another case and other spacing are still read and checked.
        let why = check("  Passwd :   files ldap\ngroup: files\n").unwrap_err();
        assert!(why.contains("`ldap`"), "{why}");
        let why = check("passwd: files\nGROUP\t: files\ngroup: files\n").unwrap_err();
        assert!(why.contains("more than once"), "{why}");
        // A line that cannot be read as `name: sources`.
        let why = check("passwd files\ngroup: files\n").unwrap_err();
        assert!(why.contains("cannot be read"), "{why}");
        assert!(check("hosts files dns\npasswd: files\ngroup: files\n").is_err());
        // A directory service on the first line.
        let why = check("passwd: files sss\ngroup: files sss\n").unwrap_err();
        assert!(why.contains("`sss`"), "{why}");
        // A non-local source on a second `passwd` line.
        let why = check("passwd: files systemd\ngroup: files\npasswd: ldap\n").unwrap_err();
        assert!(why.contains("more than once"), "{why}");
        // `initgroups` naming a directory service.
        let why = check("passwd: files\ngroup: files\ninitgroups: files winbind\n").unwrap_err();
        assert!(why.contains("`winbind`"), "{why}");
        // A database not named at all, or a line that cannot be read.
        assert!(check("passwd: files\n").is_err());
        assert!(check("passwd: files [NOTFOUND=return\ngroup: files\n").is_err());
        // Comments do not count.
        assert_eq!(
            check("# passwd: ldap\npasswd: files # ldap\ngroup: files\n"),
            Ok(())
        );
    }

    #[test]
    fn a_failed_lookup_is_refused() {
        let why = AccountDb::default().is_private(1000, 1000).unwrap_err();
        assert!(why.contains("could not be read"), "{why}");
        // No readable nsswitch.conf.
        let db = AccountDb::given(None, Some(PASSWD.into()), Some(GROUPS.into()));
        assert!(db.is_private(1000, 1000).unwrap_err().contains("nsswitch"));
        let db = AccountDb::given(Some(NSSWITCH.into()), Some(PASSWD.into()), None);
        assert!(db.is_private(1000, 1000).is_err());
        // Not listed, or a line that cannot be read.
        assert!(private(1000, 2000, PASSWD, GROUPS)
            .unwrap_err()
            .contains("not found"));
        assert!(private(1000, 1000, "me:x:abc:1000\n", GROUPS).is_err());
        assert!(private(1000, 1000, PASSWD, "me:x:1000\n").is_err());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn elsewhere_than_linux_group_write_is_not_trusted() {
        let why = AccountDb::system().is_private(20, 501).unwrap_err();
        assert!(why.contains("umask is 022"), "{why}");
    }
}
