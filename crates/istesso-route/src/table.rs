//! The role table: every paired machine as `istesso hosts --json` prints it.
//!
//! Only the id, name, whether it is this machine, its state, its role table,
//! its load and when it last answered are read; other fields are ignored.
//! The reader is tolerant in the same way as `istesso-proto`: an entry
//! without id or name is skipped, and a role table this version cannot read
//! leaves the machine in the list without one.

use std::fmt;

use istesso_proto::{HostManifest, HostState, HostStatus};
use serde_json::Value;

/// One machine as `istesso hosts --json` shows it; other fields are ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub local: bool,
    /// `Awake`: it answered just now.
    pub state: HostState,
    /// From the answer, or remembered from the last one.
    pub manifest: Option<HostManifest>,
    /// Load; only when it answered.
    pub status: Option<HostStatus>,
    /// When it last answered, in seconds since the Unix epoch.
    pub seen_at: Option<u64>,
}

impl Candidate {
    /// True when the machine can take work now: it is this machine, or it
    /// answered just now.
    pub fn answers(&self) -> bool {
        self.local || self.state == HostState::Awake
    }

    /// True when the machine could be woken over the network.
    pub fn wake_possible(&self) -> bool {
        self.manifest
            .as_ref()
            .is_some_and(HostManifest::wake_possible)
    }

    pub fn load(&self) -> Option<f32> {
        self.status.as_ref().and_then(|s| s.load)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableError {
    NotJson(String),
    NotAList,
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::NotJson(why) => write!(f, "the role table is not JSON: {why}"),
            TableError::NotAList => f.write_str(
                "the role table is not a list of machines (it is read as `istesso hosts --json` prints it)",
            ),
        }
    }
}

impl std::error::Error for TableError {}

/// An entry without id or name is skipped. A role table in a shape this
/// version cannot read leaves `manifest` empty; the machine stays in the list.
pub fn read_table(text: &str) -> Result<Vec<Candidate>, TableError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| TableError::NotJson(e.to_string()))?;
    let Value::Array(entries) = value else {
        return Err(TableError::NotAList);
    };
    Ok(entries.iter().filter_map(read_entry).collect())
}

fn read_entry(entry: &Value) -> Option<Candidate> {
    let fields = entry.as_object()?;
    let id = fields.get("id")?.as_str()?.to_owned();
    let name = fields.get("name")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let state = fields
        .get("state")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(HostState::Unknown);
    let manifest = fields
        .get("manifest")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let status = fields
        .get("status")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    Some(Candidate {
        id,
        name,
        local: fields
            .get("local")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        state,
        manifest,
        status,
        seen_at: fields.get("seen_at").and_then(Value::as_u64),
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    pub const TWO: &str = include_str!("../tests/fixtures/hosts-two.json");
    pub const THREE: &str = include_str!("../tests/fixtures/hosts-three.json");
    pub const TOWER_ASLEEP: &str = include_str!("../tests/fixtures/hosts-tower-asleep.json");
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use istesso_proto::{OsFamily, Role};

    #[test]
    fn each_fixture_parses_as_the_role_table_type() {
        // Strictly, as `istesso-proto` reads it, not through the tolerant reader.
        for (name, text) in [
            ("hosts-two", TWO),
            ("hosts-three", THREE),
            ("hosts-tower-asleep", TOWER_ASLEEP),
        ] {
            let entries: Vec<Value> = serde_json::from_str(text).expect(name);
            assert!(!entries.is_empty(), "{name}");
            for entry in entries {
                let manifest: HostManifest = serde_json::from_value(entry["manifest"].clone())
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(manifest.schema, istesso_proto::MANIFEST_SCHEMA, "{name}");
                assert_eq!(manifest.name, entry["name"].as_str().unwrap(), "{name}");
                let _: HostState = serde_json::from_value(entry["state"].clone()).expect(name);
                if !entry["status"].is_null() {
                    let _: HostStatus =
                        serde_json::from_value(entry["status"].clone()).expect(name);
                }
            }
        }
    }

    #[test]
    fn the_fixtures_are_read_with_every_role_table() {
        let two = read_table(TWO).unwrap();
        assert_eq!(
            two.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["air", "tower"]
        );
        assert!(two.iter().all(|c| c.manifest.is_some()));
        assert!(two[0].local && !two[1].local);
        let tower = two[1].manifest.as_ref().unwrap();
        assert_eq!(tower.os.family, OsFamily::Linux);
        assert!(tower.roles.contains(&Role::Gpu));
        assert!(tower.wake_possible());

        let three = read_table(THREE).unwrap();
        assert_eq!(three.len(), 3);
        assert!(three.iter().all(|c| c.manifest.is_some()));
        let with_always_on: Vec<&str> = three
            .iter()
            .filter(|c| {
                c.manifest
                    .as_ref()
                    .unwrap()
                    .roles
                    .contains(&Role::AlwaysOnCandidate)
            })
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(with_always_on, vec!["tower", "mini"]);

        let asleep = read_table(TOWER_ASLEEP).unwrap();
        assert_eq!(asleep[1].state, HostState::Unknown);
        assert!(asleep[1].status.is_none());
        assert!(asleep[1].manifest.is_some());
        assert!(!asleep[1].answers());
        assert!(asleep[0].answers());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let table = read_table(
            r#"[{"id": "a", "name": "a", "local": false, "state": "awake",
                 "manifest": null, "status": null, "seen_at": 5,
                 "colour": "blue", "address": "192.0.2.1"}]"#,
        )
        .unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].seen_at, Some(5));
        assert_eq!(table[0].state, HostState::Awake);
    }

    #[test]
    fn entries_without_id_or_name_are_skipped() {
        let table = read_table(
            r#"[{"name": "no-id"}, {"id": "x"}, {"id": "", "name": "empty"}, 7,
                {"id": "ok", "name": "ok"}]"#,
        )
        .unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].id, "ok");
        assert!(!table[0].local);
        assert_eq!(table[0].state, HostState::Unknown);
    }

    #[test]
    fn a_role_table_of_another_shape_leaves_the_machine_without_one() {
        let table = read_table(
            r#"[{"id": "a", "name": "a", "state": "awake", "manifest": {"schema": 9, "roles": "all"},
                 "status": {"load": "high"}}]"#,
        )
        .unwrap();
        assert_eq!(table.len(), 1);
        assert!(table[0].manifest.is_none());
        assert!(table[0].status.is_none());
    }

    #[test]
    fn an_unknown_state_becomes_unknown() {
        let table = read_table(r#"[{"id": "a", "name": "a", "state": "hibernating"}]"#).unwrap();
        assert_eq!(table[0].state, HostState::Unknown);
    }

    #[test]
    fn other_shapes_are_refused() {
        assert!(matches!(read_table("{"), Err(TableError::NotJson(_))));
        assert_eq!(read_table(r#"{"id": "a"}"#), Err(TableError::NotAList));
        assert!(matches!(read_table(""), Err(TableError::NotJson(_))));
    }
}
