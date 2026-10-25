//! Healing as a diff query.
//!
//! A test that follows a page breaks when the page changes. Other tools repair it by guessing
//! until it passes, which hides real defects. TraceQA only repairs what keeps the identity of the
//! component, and treats everything else as a decision for a person:
//!
//! - **Same id, another locator:** a mechanical fix ([`HealKind::LocatorFix`]).
//! - **Removed, with exactly one match on both sides:** a proposal with its evidence, never applied
//!   by itself ([`HealKind::RenameProposal`]).
//! - **Removed, no match:** a failure. **More than one match:** refused.
//! - **A run that was signed out:** nothing is healed ([`HealKind::SessionNotAccepted`]).
//!
//! Everything here is pure: it reads no files and starts no browser. `traceqa heal` supplies the
//! files, the baseline store and the page objects. See `spec/heal.schema.json`.

mod candidates;
pub mod patch;
pub mod similarity;

pub use candidates::{classify, landmark, preview, HealError, MAX_MOVE, SESSION_NOT_ACCEPTED};
use traceqa_common::types::BaselineRef;
pub use traceqa_common::types::{HealAction, HealConfidence, HealItem, HealKind, HealReport, HealSummary};

/// `HealReport.formatVersion`.
pub const FORMAT_VERSION: u64 = 1;

fn kind_order(kind: HealKind) -> u8 {
    match kind {
        HealKind::SessionNotAccepted => 0,
        HealKind::Ambiguous => 1,
        HealKind::Removed => 2,
        HealKind::RenameProposal => 3,
        HealKind::LocatorFix => 4,
        HealKind::Unchanged => 5,
    }
}

/// Does this item make the command exit 1? Anything that is not `none` or `fix`, unless a person
/// accepted it.
pub fn is_failure(item: &HealItem) -> bool {
    !matches!(item.action, HealAction::None | HealAction::Fix) && item.accepted != Some(true)
}

/// The report for the items of every target: unchanged items are counted and left out, the rest are
/// ordered by target, kind and id.
pub fn build_report(run_id: &str, baseline: BaselineRef, mut items: Vec<HealItem>) -> HealReport {
    let count = |kind: HealKind| items.iter().filter(|i| i.kind == kind).count() as u64;
    let accepted = items
        .iter()
        .filter(|i| i.kind != HealKind::Unchanged && i.accepted == Some(true))
        .count() as u64;
    let mut summary = HealSummary {
        unchanged: count(HealKind::Unchanged),
        locator_fixes: count(HealKind::LocatorFix),
        rename_proposals: count(HealKind::RenameProposal),
        removed: count(HealKind::Removed),
        ambiguous: count(HealKind::Ambiguous),
        session_not_accepted: count(HealKind::SessionNotAccepted),
        accepted: (accepted > 0).then_some(accepted),
        failed: false,
    };
    items.retain(|i| i.kind != HealKind::Unchanged);
    items.sort_by(|a, b| (&a.target, kind_order(a.kind), &a.id).cmp(&(&b.target, kind_order(b.kind), &b.id)));
    summary.failed = items.iter().any(is_failure);
    HealReport {
        format_version: FORMAT_VERSION,
        run_id: run_id.to_string(),
        baseline,
        items,
        summary,
    }
}

/// The report as Markdown for a person or a pull request comment. Page text is shown as plain text
/// cut to a safe length, in code spans.
pub fn render_markdown(report: &HealReport) -> String {
    use std::fmt::Write as _;
    use traceqa_diff::text::code_span;

    let mut out = String::from("## TraceQA heal\n\n");
    let s = &report.summary;
    let _ = writeln!(
        out,
        "Run {} against baseline {}: {} unchanged, {} locator fixes, {} rename proposals, {} removed, {} ambiguous{}.\n",
        code_span(&report.run_id),
        code_span(&report.baseline.key),
        s.unchanged,
        s.locator_fixes,
        s.rename_proposals,
        s.removed,
        s.ambiguous,
        if s.session_not_accepted > 0 { ", session not accepted" } else { "" }
    );
    if report.items.is_empty() {
        out.push_str("Nothing to heal.\n");
        return out;
    }
    for item in &report.items {
        let what = match item.kind {
            HealKind::LocatorFix => "Locator fix",
            HealKind::RenameProposal => "Rename proposal",
            HealKind::Removed => "Removed",
            HealKind::Ambiguous => "Ambiguous, refused",
            HealKind::SessionNotAccepted => "Session not accepted",
            HealKind::Unchanged => "Unchanged",
        };
        let mut title = format!("### {what}");
        if !item.id.is_empty() {
            let _ = write!(title, " {}", code_span(&item.id));
        }
        if let Some(new_id) = &item.new_id {
            if *new_id != item.id {
                let _ = write!(title, " to {}", code_span(new_id));
            }
        }
        if let Some(confidence) = item.confidence {
            let _ = write!(title, " ({confidence} confidence)");
        }
        if item.accepted == Some(true) {
            title.push_str(", accepted");
        }
        let _ = writeln!(out, "{title}\n");
        if let (Some(role), Some(name)) = (&item.role, &item.name) {
            let _ = writeln!(out, "{} {}\n", code_span(role), code_span(name));
        }
        for line in &item.evidence {
            let _ = writeln!(out, "- {}", code_span(line));
        }
        if let Some(preview) = &item.fix_preview {
            let _ = writeln!(out, "\n```diff\n{}\n```", preview.replace("```", "'''"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
