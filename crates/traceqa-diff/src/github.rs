//! GitHub Actions workflow commands (`::error title=...::message`) for a diff or for findings.
//!
//! Page text is untrusted, so every message goes through [`text::plain`] and then the escaping of
//! the Actions toolkit (`escapeData`, `escapeProperty`): `%`, CR and LF in data, and also `:` and `,`
//! in properties. A newline in a name can therefore not start another command. GitHub shows at most
//! 10 errors and 10 warnings per step and 50 annotations per job, so [`render`] writes at most
//! [`MAX_ANNOTATIONS`] lines, most serious first; the full list is in the step summary
//! (the Markdown report).

use std::fmt::Write as _;

use traceqa_common::types::{DiffEntry, DiffEntryKind, DiffFile, FindingSeverity, FindingsFile};

use crate::gate::{classify, reportable, Class, Severity};
use crate::text::plain;

/// The most workflow commands written, the closing notice included.
pub const MAX_ANNOTATIONS: usize = 50;

/// One workflow command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub severity: Severity,
    /// Becomes `title=TraceQA <title>`.
    pub title: String,
    pub message: String,
}

/// The Actions toolkit's `escapeData`.
pub fn escape_data(text: &str) -> String {
    text.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

/// The Actions toolkit's `escapeProperty`.
pub fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

/// Writes the commands, one per line, errors first and otherwise in the given order. More than
/// [`MAX_ANNOTATIONS`] are cut, and the last line says how many were left out.
pub fn render(items: &[Annotation]) -> String {
    let mut sorted: Vec<&Annotation> = items.iter().collect();
    sorted.sort_by_key(|a| a.severity); // stable
    let mut out = String::new();
    let shown = if sorted.len() > MAX_ANNOTATIONS {
        MAX_ANNOTATIONS - 1
    } else {
        sorted.len()
    };
    for item in &sorted[..shown] {
        let command = match item.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "notice",
        };
        let _ = writeln!(
            out,
            "::{command} title={}::{}",
            escape_property(&format!("TraceQA {}", item.title)),
            escape_data(&item.message)
        );
    }
    if sorted.len() > shown {
        let _ = writeln!(
            out,
            "::notice title={}::{}",
            escape_property("TraceQA"),
            escape_data(&format!(
                "{} more not annotated; see the step summary or use --format json",
                sorted.len() - shown
            ))
        );
    }
    out
}

fn readable(flag: &str) -> String {
    flag.replace('_', " ")
}

/// One sentence about a diff entry, with the page's words cleaned and cut.
pub(crate) fn entry_message(target: &str, entry: &DiffEntry) -> String {
    let summary = entry.b.as_ref().or(entry.a.as_ref());
    let what = match summary {
        Some(s) => format!(
            "{} \"{}\" ({})",
            plain(&s.role),
            if s.name_is_template {
                "*".to_string()
            } else {
                plain(&s.name)
            },
            entry.id
        ),
        None => entry.id.clone(),
    };
    let other = entry.paired_with.as_ref().map(|p| (p.id.as_str(), p.reason.as_str()));
    let mut text = match (classify(entry), entry.kind) {
        (Class::Renamed, _) => format!("{what} was renamed to {}", other.map_or("?", |o| o.0)),
        (_, DiffEntryKind::Removed) => format!("{what} is in the baseline but not in the run"),
        (_, DiffEntryKind::Added) => format!("{what} is in the run but not in the baseline"),
        (_, DiffEntryKind::Changed) => {
            let mut parts: Vec<String> = entry.flags.iter().map(|f| readable(f)).collect();
            if parts.is_empty() {
                parts = entry.fields.clone();
            }
            format!("{what} changed: {}", parts.join(", "))
        }
    };
    if let (Some((id, reason)), false) = (other, classify(entry) == Class::Renamed) {
        let _ = write!(text, "; looks like {id} ({})", plain(&reason.replace('_', " ")));
    }
    format!("{}: {text}", plain(target))
}

/// Annotations for the diffs of the targets of a run. A target that could not be compared is an
/// error annotation.
pub fn diff_annotations(outcomes: &[(&str, Result<&DiffFile, &str>)]) -> Vec<Annotation> {
    let mut items = Vec::new();
    for (target, outcome) in outcomes {
        match outcome {
            Err(why) => items.push(Annotation {
                severity: Severity::Error,
                title: "target missing".into(),
                message: format!("{}: {}", plain(target), plain(why)),
            }),
            Ok(diff) => {
                for entry in reportable(diff) {
                    let class = classify(entry);
                    items.push(Annotation {
                        severity: class.severity(),
                        title: class.name().into(),
                        message: entry_message(target, entry),
                    });
                }
            }
        }
    }
    items
}

/// Annotations for findings: error, warning and info become error, warning and notice.
pub fn finding_annotations(files: &[&FindingsFile]) -> Vec<Annotation> {
    let mut items = Vec::new();
    for file in files {
        let target = file.target.as_deref().unwrap_or("run");
        for f in &file.findings {
            items.push(Annotation {
                severity: match f.severity {
                    FindingSeverity::Error => Severity::Error,
                    FindingSeverity::Warning => Severity::Warning,
                    FindingSeverity::Info => Severity::Note,
                },
                title: plain(&f.rule),
                message: format!(
                    "{}: {} \"{}\" ({}): {}",
                    plain(target),
                    plain(&f.role),
                    plain(&f.name),
                    f.component_id,
                    plain(&f.message)
                ),
            });
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(severity: Severity, message: &str) -> Annotation {
        Annotation {
            severity,
            title: "removed".into(),
            message: message.into(),
        }
    }

    #[test]
    fn data_and_properties_are_escaped_as_the_toolkit_does() {
        assert_eq!(escape_data("100%\r\nx: y, z"), "100%25%0D%0Ax: y, z");
        assert_eq!(escape_property("a:b,c%\n"), "a%3Ab%2Cc%25%0A");
    }

    #[test]
    fn a_hostile_name_cannot_start_another_command() {
        let out = render(&[note(Severity::Error, "x\n::error title=pwned::boom")]);
        assert_eq!(out.lines().count(), 1);
        assert!(out.starts_with("::error title=TraceQA removed::"));
        assert!(out.contains("%0A::error title=pwned::boom"));
    }

    #[test]
    fn at_most_fifty_lines_with_errors_first() {
        let mut items: Vec<Annotation> = (0..70).map(|i| note(Severity::Warning, &format!("w{i}"))).collect();
        items.push(note(Severity::Error, "last but worst"));
        let out = render(&items);
        assert_eq!(out.lines().count(), MAX_ANNOTATIONS);
        assert!(out.lines().next().unwrap().contains("last but worst"));
        assert!(out.lines().last().unwrap().starts_with("::notice"));
        assert!(out.contains("22 more"));
    }

    #[test]
    fn exactly_fifty_are_all_shown() {
        let items: Vec<Annotation> = (0..50).map(|i| note(Severity::Note, &format!("n{i}"))).collect();
        let out = render(&items);
        assert_eq!(out.lines().count(), 50);
        assert!(!out.contains("more not annotated"));
    }
}
