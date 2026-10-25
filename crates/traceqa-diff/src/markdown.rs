//! The Markdown report, made for a pull request comment or a CI step summary.
//!
//! Every string that came from a page or a file goes through [`code_span`]; the rest is fixed
//! text and numbers. Each table is cut at [`MAX_ROWS`] rows, which also keeps the report far
//! below GitHub's limit of 1 MiB for a step summary (a row is well under 1 KiB).

use std::fmt::Write as _;

use traceqa_common::types::{ComponentSummary, DiffEntry, DiffEntryKind, DiffFile, DiffSide};

use crate::text::{clean, code_span};

/// The most rows shown in each of the Removed, Added and Changed tables.
pub const MAX_ROWS: usize = 50;

/// Renders the diff. Ends with a newline.
pub fn render_markdown(diff: &DiffFile) -> String {
    let mut out = String::new();
    out.push_str("## TraceQA diff\n\n");
    let _ = writeln!(out, "- **A**: {}", describe(&diff.a));
    let _ = writeln!(out, "- **B**: {}", describe(&diff.b));
    let _ = writeln!(out, "- **Mode**: {}\n", code_span(&diff.mode));

    let s = &diff.summary;
    let _ = writeln!(
        out,
        "**{} removed, {} added, {} changed, {} unchanged, {} paired**\n",
        s.removed, s.added, s.changed, s.unchanged, s.paired
    );
    for warning in &diff.warnings {
        let _ = writeln!(
            out,
            "> **Note** ({}): {}\n",
            code_span(&warning.code),
            clean(&warning.message)
        );
    }
    if diff.entries.is_empty() {
        out.push_str("No differences.\n");
        return out;
    }
    for (kind, title) in [
        (DiffEntryKind::Removed, "Removed (in A only)"),
        (DiffEntryKind::Added, "Added (in B only)"),
        (DiffEntryKind::Changed, "Changed"),
    ] {
        let rows: Vec<&DiffEntry> = diff.entries.iter().filter(|e| e.kind == kind).collect();
        if !rows.is_empty() {
            table(&mut out, title, kind, &rows);
        }
    }
    out
}

fn describe(side: &DiffSide) -> String {
    let mut text = code_span(&side.label);
    if let Some(target) = &side.target {
        let _ = write!(text, " target {}", code_span(target));
    }
    let _ = write!(
        text,
        ", {} {}, {}x{}",
        code_span(&side.browser.name),
        code_span(&side.browser.version),
        side.viewport.width,
        side.viewport.height
    );
    let _ = write!(text, ", {} components", side.component_count);
    text
}

fn table(out: &mut String, title: &str, kind: DiffEntryKind, rows: &[&DiffEntry]) {
    let _ = writeln!(out, "### {title} ({})\n", rows.len());
    let last_column = if kind == DiffEntryKind::Changed {
        "Changes"
    } else {
        "Paired with"
    };
    let _ = writeln!(out, "| Id | Role | Name | Path | {last_column} |");
    out.push_str("|---|---|---|---|---|\n");
    for entry in rows.iter().take(MAX_ROWS) {
        let Some(summary) = entry.b.as_ref().or(entry.a.as_ref()) else {
            continue;
        };
        let last = if kind == DiffEntryKind::Changed {
            changes(entry)
        } else {
            pairing(entry)
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            code_span(&entry.id),
            code_span(&summary.role),
            name(summary),
            code_span(&summary.path),
            last
        );
    }
    if rows.len() > MAX_ROWS {
        let _ = writeln!(out, "\nand {} more, use --format json", rows.len() - MAX_ROWS);
    }
    out.push('\n');
}

fn name(summary: &ComponentSummary) -> String {
    if summary.name_is_template {
        format!("{} (template)", code_span(&summary.name))
    } else {
        code_span(&summary.name)
    }
}

fn pairing(entry: &DiffEntry) -> String {
    match &entry.paired_with {
        Some(link) => format!(
            "{} ({}, {} confidence)",
            code_span(&link.id),
            clean(&link.reason).replace('|', " "),
            link.confidence
        ),
        None => String::new(),
    }
}

fn changes(entry: &DiffEntry) -> String {
    let (Some(a), Some(b)) = (&entry.a, &entry.b) else {
        return String::new();
    };
    let values = |field: &str| -> Option<(String, String)> {
        let (x, y) = match field {
            "instances" => (a.instances, b.instances),
            "visible" => (a.visible, b.visible),
            "enabled" => (a.enabled, b.enabled),
            "covered" => (a.covered, b.covered),
            "checked" => (a.checked, b.checked),
            "expanded" => (a.expanded, b.expanded),
            "selected" => (a.selected, b.selected),
            "pressed" => (a.pressed, b.pressed),
            "confidence" => return Some((a.confidence.to_string(), b.confidence.to_string())),
            _ => return None,
        };
        Some((x.to_string(), y.to_string()))
    };
    entry
        .fields
        .iter()
        .filter_map(|field| {
            let (x, y) = values(field)?;
            Some(format!("{field} {x} \u{2192} {y}"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}
