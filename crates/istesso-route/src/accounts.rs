//! Whether a group can hold anyone but the user who runs the command.
//!
//! A task file, or a folder leading to it, that its group can write is
//! accepted only when the group is the user's own private group: the group
//! named exactly like the user, with no members listed, that no other
//! account has as its primary group. Ubuntu's default umask (002) makes
//! every file group-writable, and there the group is such a private one.
//!
//! The answer comes from the system's own files, read as text: `/etc/passwd`,
//! `/etc/group` and `/etc/nsswitch.conf`. When it cannot be established —
//! a file is missing or unreadable, the user is not listed, or accounts may
//! come from elsewhere (a directory service) — the group counts as shared.
//! On macOS accounts live in Directory Services, so the answer is "shared".

/// The account databases, as text. `None`: the file could not be read.
#[derive(Debug, Clone, Default)]
pub(crate) struct AccountDb {
    pub passwd: Option<String>,
    pub group: Option<String>,
    pub nsswitch: Option<String>,
}

impl AccountDb {
    /// The system's own files.
    pub(crate) fn system() -> AccountDb {
        let read = |path: &str| std::fs::read_to_string(path).ok();
        AccountDb {
            passwd: read("/etc/passwd"),
            group: read("/etc/group"),
            nsswitch: read("/etc/nsswitch.conf"),
        }
    }

    /// The name of the group with this id, for messages; the number if unknown.
    pub(crate) fn group_name(&self, gid: u32) -> String {
        self.group
            .as_deref()
            .and_then(|text| {
                entries(text, 4)
                    .find(|f| f[2].parse() == Ok(gid))
                    .map(|f| f[0].to_owned())
            })
            .unwrap_or_else(|| gid.to_string())
    }
}

/// The lines of a colon-separated database with at least `fields` fields.
fn entries(text: &str, fields: usize) -> impl Iterator<Item = Vec<&str>> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').collect::<Vec<_>>())
        .filter(move |f| f.len() >= fields)
}

/// Sources that keep accounts in the local files only.
const LOCAL_SOURCES: [&str; 3] = ["files", "compat", "systemd"];

/// `Ok` when the group `gid` cannot hold anyone but the user `uid`; else why
/// it may.
pub(crate) fn group_is_private(gid: u32, uid: u32, db: &AccountDb) -> Result<(), String> {
    let (Some(passwd), Some(group)) = (db.passwd.as_deref(), db.group.as_deref()) else {
        return Err("the account files cannot be read".into());
    };
    let Some(nsswitch) = db.nsswitch.as_deref() else {
        return Err("it cannot be told where accounts come from (no /etc/nsswitch.conf)".into());
    };
    for database in ["passwd", "group"] {
        let line = nsswitch
            .lines()
            .map(|l| l.split('#').next().unwrap_or_default().trim())
            .find(|l| l.split(':').next().map(str::trim) == Some(database));
        let Some(line) = line else {
            return Err(format!(
                "/etc/nsswitch.conf does not say where `{database}` comes from"
            ));
        };
        let sources = line.split_once(':').map(|(_, s)| s).unwrap_or_default();
        for source in sources.split_whitespace().filter(|s| !s.starts_with('[')) {
            if !LOCAL_SOURCES.contains(&source) {
                return Err(format!(
                    "accounts may come from `{source}`, which this command cannot read"
                ));
            }
        }
    }
    if passwd
        .lines()
        .chain(group.lines())
        .any(|l| l.starts_with('+') || l.starts_with('-'))
    {
        return Err("the account files include entries from elsewhere (`+`/`-` lines)".into());
    }

    let mut users = entries(passwd, 4).filter(|f| f[2].parse() == Ok(uid));
    let Some(user) = users.next() else {
        return Err("the user who runs the command is not listed in /etc/passwd".into());
    };
    let name = user[0];
    if users.next().is_some() {
        return Err("two accounts share the user's id".into());
    }

    let named: Vec<Vec<&str>> = entries(group, 4).filter(|f| f[0] == name).collect();
    let [own] = named.as_slice() else {
        return Err(format!("there is not exactly one group named `{name}`"));
    };
    if own[2].parse() != Ok(gid) {
        return Err(format!(
            "it is not the group named like the user (`{name}`)"
        ));
    }
    let members = own[3].trim();
    if !members.is_empty() {
        return Err(format!("the group lists members ({members})"));
    }
    if entries(group, 4).any(|f| f[2].parse() == Ok(gid) && f[0] != name) {
        return Err("another group has the same id".into());
    }
    if let Some(other) = entries(passwd, 4).find(|f| f[3].parse() == Ok(gid) && f[0] != name) {
        return Err(format!(
            "it is the primary group of the account `{}`",
            other[0]
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A database where `me` (the given uid) has the private group `me`
    /// (the given gid). The other accounts and groups are left out when they
    /// would share that id (in a temporary folder on macOS, files get the
    /// group of id 0).
    pub(crate) fn private_db(uid: u32, gid: u32) -> AccountDb {
        let mut passwd = String::new();
        let mut group = String::new();
        if uid != 0 && gid != 0 {
            passwd.push_str("root:x:0:0:root:/root:/bin/sh\n");
            group.push_str("root:x:0:\n");
        }
        if gid != 4 {
            group.push_str("adm:x:4:syslog\n");
        }
        passwd.push_str(&format!("me:x:{uid}:{gid}:Me:/nonexistent:/bin/sh\n"));
        group.push_str(&format!("me:x:{gid}:\n"));
        AccountDb {
            passwd: Some(passwd),
            group: Some(group),
            nsswitch: Some(
                "passwd: files systemd\ngroup: files systemd\nhosts: files dns\n".into(),
            ),
        }
    }

    #[test]
    fn the_user_s_private_group_is_private() {
        assert_eq!(
            group_is_private(1000, 1000, &private_db(1000, 1000)),
            Ok(())
        );
    }

    #[test]
    fn a_group_with_another_member_is_shared() {
        let mut db = private_db(1000, 1000);
        db.group = Some("me:x:1000:guest\n".into());
        let why = group_is_private(1000, 1000, &db).unwrap_err();
        assert!(why.contains("members (guest)"), "{why}");
    }

    #[test]
    fn a_group_that_is_another_account_s_primary_group_is_shared() {
        let mut db = private_db(1000, 1000);
        db.passwd = Some(
            "me:x:1000:1000::/nonexistent:/bin/sh\nguest:x:1001:1000::/nonexistent:/bin/sh\n"
                .into(),
        );
        let why = group_is_private(1000, 1000, &db).unwrap_err();
        assert!(why.contains("`guest`"), "{why}");
    }

    #[test]
    fn another_group_than_the_user_s_own_is_shared() {
        // `staff` on macOS, or any group named otherwise.
        let why = group_is_private(20, 1000, &private_db(1000, 1000)).unwrap_err();
        assert!(why.contains("named like the user"), "{why}");
    }

    #[test]
    fn an_answer_that_cannot_be_established_is_shared() {
        // Files that cannot be read.
        assert!(group_is_private(1000, 1000, &AccountDb::default()).is_err());
        let mut db = private_db(1000, 1000);
        db.group = None;
        assert!(group_is_private(1000, 1000, &db).is_err());
        // No nsswitch.conf (as on macOS), or accounts from a directory service.
        let mut db = private_db(1000, 1000);
        db.nsswitch = None;
        assert!(group_is_private(1000, 1000, &db).is_err());
        let mut db = private_db(1000, 1000);
        db.nsswitch = Some("passwd: files sss\ngroup: files sss\n".into());
        let why = group_is_private(1000, 1000, &db).unwrap_err();
        assert!(why.contains("`sss`"), "{why}");
        let mut db = private_db(1000, 1000);
        db.nsswitch = Some("passwd: files\n".into());
        assert!(group_is_private(1000, 1000, &db).is_err());
        // The user is not listed (accounts elsewhere).
        let why = group_is_private(1000, 2000, &private_db(1000, 1000)).unwrap_err();
        assert!(why.contains("not listed"), "{why}");
        // Entries pulled in from elsewhere.
        let mut db = private_db(1000, 1000);
        db.group = Some("me:x:1000:\n+:::\n".into());
        assert!(group_is_private(1000, 1000, &db).is_err());
    }

    #[test]
    fn the_group_s_name_is_found_for_messages() {
        let db = private_db(1000, 1000);
        assert_eq!(db.group_name(4), "adm");
        assert_eq!(db.group_name(77), "77");
    }
}
