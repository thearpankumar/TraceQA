//! The suppression file: findings a team has looked at and decided to live with.
//!
//! ```yaml
//! suppressions:
//!   - id: small-target:c_9f2a41c07b3e      # or: rule: small-target + component: c_9f2a41c07b3e
//!     reason: Inline link inside a sentence (WCAG 2.5.8 inline exception).
//!     expires: 2027-01-31                  # optional, YYYY-MM-DD, applies through that day
//! ```
//!
//! The file is user input: unknown keys are refused, `reason` is required, and a file larger
//! than [`MAX_FILE_BYTES`] is refused. An entry that matches nothing is reported, as ESLint does
//! for unused suppressions, so the file does not rot.

use std::collections::HashSet;

use serde::Deserialize;
use traceqa_common::types::Warning;

use crate::clean;

/// Larger files are refused before they are parsed.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// More entries than this are refused.
pub const MAX_ENTRIES: usize = 2000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    suppressions: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    id: Option<String>,
    rule: Option<String>,
    component: Option<String>,
    reason: Option<String>,
    expires: Option<String>,
}

/// One entry of the file, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The finding id it matches: `rule:component`.
    pub id: String,
    pub reason: String,
    /// `YYYY-MM-DD`, validated.
    pub expires: Option<String>,
}

/// The parsed file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suppressions {
    pub entries: Vec<Entry>,
}

/// Why the file could not be used. The message names the entry.
#[derive(Clone, Debug, PartialEq)]
pub struct SuppressionError(pub String);

impl std::fmt::Display for SuppressionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SuppressionError {}

fn err<T>(message: impl Into<String>) -> Result<T, SuppressionError> {
    Err(SuppressionError(message.into()))
}

/// True for `YYYY-MM-DD` naming a day that exists.
pub fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = &text[range];
        if part.bytes().all(|b| b.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let longest = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=longest).contains(&day)
}

/// Reads the file. An empty file is an empty list.
pub fn parse(text: &str) -> Result<Suppressions, SuppressionError> {
    if text.len() > MAX_FILE_BYTES {
        return err(format!(
            "the suppression file is larger than {} KiB",
            MAX_FILE_BYTES / 1024
        ));
    }
    if text.trim().is_empty() {
        return Ok(Suppressions::default());
    }
    let raw: RawFile = serde_norway::from_str(text)
        .map_err(|e| SuppressionError(format!("the suppression file is not valid: {e}")))?;
    if raw.suppressions.len() > MAX_ENTRIES {
        return err(format!("the suppression file has more than {MAX_ENTRIES} entries"));
    }
    let mut entries = Vec::new();
    for (index, entry) in raw.suppressions.into_iter().enumerate() {
        let number = index + 1;
        let id = match (entry.id, entry.rule, entry.component) {
            (Some(id), None, None) => id.trim().to_string(),
            (None, Some(rule), Some(component)) => format!("{}:{}", rule.trim(), component.trim()),
            _ => {
                return err(format!(
                    "suppression {number}: give either `id`, or `rule` together with `component`"
                ))
            }
        };
        if !id.contains(':') || id.starts_with(':') || id.ends_with(':') {
            return err(format!(
                "suppression {number}: `{}` is not a finding id (rule:component)",
                clean(&id)
            ));
        }
        let reason = entry.reason.map(|r| r.trim().to_string()).unwrap_or_default();
        if reason.is_empty() {
            return err(format!("suppression {number} ({}): `reason` is required", clean(&id)));
        }
        if let Some(date) = &entry.expires {
            if !is_date(date) {
                return err(format!(
                    "suppression {number} ({}): `expires` must be a date as YYYY-MM-DD, not `{}`",
                    clean(&id),
                    clean(date)
                ));
            }
        }
        entries.push(Entry {
            id,
            reason: clean(&reason),
            expires: entry.expires,
        });
    }
    Ok(Suppressions { entries })
}

impl Entry {
    /// True when `today` (YYYY-MM-DD) is after the day the entry applies through.
    pub fn expired(&self, today: Option<&str>) -> bool {
        matches!((&self.expires, today), (Some(expires), Some(today)) if expires.as_str() < today)
    }
}

impl Suppressions {
    /// The first entry that applies to the finding id today, as its index.
    pub(crate) fn find(&self, finding_id: &str, today: Option<&str>) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.id == finding_id && !e.expired(today))
    }
}

/// Which entries matched something, across every file of a run.
#[derive(Default)]
pub(crate) struct Usage {
    used: HashSet<usize>,
}

impl Usage {
    pub(crate) fn mark(&mut self, entry: usize) {
        self.used.insert(entry);
    }

    /// One warning per expired entry and per entry that matched nothing.
    pub(crate) fn warnings(&self, suppressions: &Suppressions, today: Option<&str>) -> Vec<Warning> {
        let mut out = Vec::new();
        for (index, entry) in suppressions.entries.iter().enumerate() {
            if entry.expired(today) {
                out.push(Warning {
                    code: "suppression_expired".into(),
                    message: format!(
                        "The suppression of {} expired on {} and is ignored. Fix the finding or renew the entry.",
                        clean(&entry.id),
                        entry.expires.as_deref().unwrap_or("")
                    ),
                });
            } else if !self.used.contains(&index) {
                out.push(Warning {
                    code: "suppression_unused".into(),
                    message: format!(
                        "The suppression of {} matched no finding. Remove the entry if the problem is fixed.",
                        clean(&entry.id)
                    ),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_id_and_rule_component_forms() {
        let file = parse(
            "suppressions:\n  - id: small-target:c_aaaaaaaaaaaa\n    reason: inline link\n  - rule: fake-button\n    component: c_bbbbbbbbbbbb\n    reason: ok\n    expires: 2027-02-28\n",
        )
        .unwrap();
        assert_eq!(file.entries[0].id, "small-target:c_aaaaaaaaaaaa");
        assert_eq!(file.entries[1].id, "fake-button:c_bbbbbbbbbbbb");
        assert_eq!(file.entries[1].expires.as_deref(), Some("2027-02-28"));
    }

    #[test]
    fn a_missing_reason_is_an_error() {
        let e = parse("suppressions:\n  - id: small-target:c_a\n").unwrap_err();
        assert!(e.0.contains("reason"), "{e}");
        let e = parse("suppressions:\n  - id: small-target:c_a\n    reason: '  '\n").unwrap_err();
        assert!(e.0.contains("reason"), "{e}");
    }

    #[test]
    fn unknown_keys_bad_dates_and_bad_shapes_are_errors() {
        assert!(parse("suppressions:\n  - id: a:b\n    reason: x\n    why: y\n").is_err());
        assert!(parse("other: 1\n").is_err());
        assert!(parse("suppressions:\n  - id: a:b\n    reason: x\n    expires: 2026-02-30\n").is_err());
        assert!(parse("suppressions:\n  - id: a:b\n    rule: a\n    component: b\n    reason: x\n").is_err());
        assert!(parse("suppressions:\n  - rule: a\n    reason: x\n").is_err());
        assert!(parse("suppressions:\n  - id: nocolon\n    reason: x\n").is_err());
    }

    #[test]
    fn a_huge_file_is_refused_and_an_empty_one_is_empty() {
        assert!(parse(&" ".repeat(MAX_FILE_BYTES + 1)).is_err());
        assert_eq!(parse("  \n").unwrap(), Suppressions::default());
    }

    #[test]
    fn dates() {
        assert!(is_date("2028-02-29"));
        assert!(!is_date("2027-02-29"));
        assert!(!is_date("2027-13-01"));
        assert!(!is_date("27-1-1"));
        assert!(!is_date("2027-01-0x"));
    }

    #[test]
    fn expiry_is_through_the_named_day() {
        let entry = Entry {
            id: "a:b".into(),
            reason: "r".into(),
            expires: Some("2027-01-31".into()),
        };
        assert!(!entry.expired(Some("2027-01-31")));
        assert!(entry.expired(Some("2027-02-01")));
        assert!(!entry.expired(None));
    }
}
