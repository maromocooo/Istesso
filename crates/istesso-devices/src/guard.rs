//! Lets through only the reports an adapter declared.
//!
//! The adapter says beforehand which report IDs and command bytes it uses.
//! Anything else is stopped here, before it reaches the device. This narrows
//! what a reviewer has to read: a write that is not in the declaration cannot
//! happen, whatever the code says.

use istesso_device_api::{ReportKind, ReportRule};

/// Whether `report` is covered by one of `rules`.
///
/// `report[0]` is the report ID. When a rule names a command byte, `report[1]`
/// must be that byte.
pub fn allows(rules: &[ReportRule], kind: ReportKind, report: &[u8]) -> bool {
    let Some(&id) = report.first() else {
        return false;
    };
    rules.iter().any(|rule| {
        rule.kind == kind
            && rule.id == id
            && match rule.command {
                None => true,
                Some(command) => report.get(1) == Some(&command),
            }
    })
}

/// What is wrong with a declaration, if anything. Checked for every adapter
/// that is built in.
pub fn check_rules(rules: &[ReportRule]) -> Result<(), String> {
    for rule in rules {
        let writes = matches!(rule.kind, ReportKind::Output | ReportKind::FeatureSet);
        if writes && rule.command.is_none() {
            return Err(format!(
                "report {:#04x} ({:?}) is written without naming its command byte",
                rule.id, rule.kind
            ));
        }
        if rule.purpose.trim().is_empty() {
            return Err(format!(
                "report {:#04x} ({:?}) does not say what it is for",
                rule.id, rule.kind
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &[ReportRule] = &[
        ReportRule {
            kind: ReportKind::Output,
            id: 0x02,
            command: Some(0x07),
            purpose: "picture",
        },
        ReportRule {
            kind: ReportKind::FeatureSet,
            id: 0x03,
            command: Some(0x08),
            purpose: "brightness",
        },
        ReportRule {
            kind: ReportKind::FeatureGet,
            id: 0x05,
            command: None,
            purpose: "version",
        },
    ];

    #[test]
    fn a_declared_report_passes() {
        assert!(allows(RULES, ReportKind::Output, &[0x02, 0x07, 0x00]));
        assert!(allows(RULES, ReportKind::FeatureSet, &[0x03, 0x08, 50]));
        assert!(allows(RULES, ReportKind::FeatureGet, &[0x05]));
    }

    #[test]
    fn the_same_id_with_another_command_is_stopped() {
        // Same report ID as brightness, another command: the sleep timer.
        assert!(!allows(
            RULES,
            ReportKind::FeatureSet,
            &[0x03, 0x0d, 0, 0, 0, 0]
        ));
        // Same report ID as a button picture, another command.
        assert!(!allows(RULES, ReportKind::Output, &[0x02, 0x08, 0x00]));
    }

    #[test]
    fn the_kind_matters() {
        // Declared as a feature report, sent as an output report.
        assert!(!allows(RULES, ReportKind::Output, &[0x03, 0x08, 50]));
        // Declared for writing, asked for reading.
        assert!(!allows(RULES, ReportKind::FeatureGet, &[0x03]));
        // The serial number was never declared.
        assert!(!allows(RULES, ReportKind::FeatureGet, &[0x06]));
    }

    #[test]
    fn an_empty_or_short_report_is_stopped() {
        assert!(!allows(RULES, ReportKind::Output, &[]));
        assert!(!allows(RULES, ReportKind::Output, &[0x02]));
    }

    #[test]
    fn a_write_must_name_its_command() {
        let rules = [ReportRule {
            kind: ReportKind::Output,
            id: 0x02,
            command: None,
            purpose: "everything",
        }];
        assert!(check_rules(&rules).is_err());
        assert!(check_rules(RULES).is_ok());
    }
}
