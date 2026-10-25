//! Classifying the components of a baseline against a run.

use std::collections::{BTreeMap, BTreeSet};

use traceqa_common::types::{
    BaselineAlias, Component, ComponentsFile, HealAction, HealConfidence, HealItem, HealKind, Locator,
};
use traceqa_diff::baseline::apply_aliases;
use traceqa_diff::{compare, DiffEntryKind, DiffError, Options};

use crate::similarity::{name_similarity, NAME_THRESHOLD};

/// How far apart the centres of two boxes may be, as a share of the viewport, on each axis.
pub const MAX_MOVE: f64 = 0.10;

/// The `warnings[].code` that says the stored session was not accepted.
pub const SESSION_NOT_ACCEPTED: &str = "session_not_accepted";

/// Landmark roles, the same list that `spec/fingerprint.md` keeps names for.
const LANDMARKS: [&str; 8] = [
    "banner",
    "main",
    "navigation",
    "contentinfo",
    "complementary",
    "region",
    "form",
    "search",
];

/// Why a run cannot be healed against a baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealError {
    /// Ids made by different recipes mean different things.
    FingerprintVersion { baseline: u64, run: u64 },
}

impl std::fmt::Display for HealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::FingerprintVersion { baseline, run } = self;
        write!(
            f,
            "the baseline uses fingerprint version {baseline} and the run {run}, so their component ids cannot be \
             compared; re-save the baseline from a current run"
        )
    }
}

impl std::error::Error for HealError {}

/// The first landmark step of a component path, such as `main` or `navigation "contents"`. Empty
/// when the path has none.
pub fn landmark(path: &str) -> &str {
    path.split(" > ")
        .find(|step| {
            let role = step.split([' ', '"']).next().unwrap_or("");
            LANDMARKS.contains(&role)
        })
        .unwrap_or("")
}

/// The `data-testid` of the first instance, when it has a non-empty one.
fn test_id(component: &Component) -> Option<&str> {
    component
        .instances
        .first()
        .and_then(|i| i.attrs.get("data-testid"))
        .map(String::as_str)
        .filter(|id| !id.is_empty())
}

/// The distance between the centres of the first instances, as a share of the viewport on each axis.
fn centre_move(a: &Component, b: &Component, run: &ComponentsFile) -> Option<(f64, f64)> {
    let (a, b) = (&a.instances.first()?.box_, &b.instances.first()?.box_);
    let (w, h) = (run.viewport.width as f64, run.viewport.height as f64);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let dx = ((a.x + a.w / 2.0) - (b.x + b.w / 2.0)).abs() / w;
    let dy = ((a.y + a.h / 2.0) - (b.y + b.h / 2.0)).abs() / h;
    Some((dx, dy))
}

/// What makes `new` a candidate for `old`: nothing when it is not one.
struct Evidence {
    confidence: HealConfidence,
    lines: Vec<String>,
}

/// Same role and landmark are required. Then either the same test id (strong), or a close name and a
/// box that stayed about where it was (medium). A different role is never a candidate.
fn candidate(old: &Component, new: &Component, run: &ComponentsFile) -> Option<Evidence> {
    if old.role != new.role || landmark(&old.path) != landmark(&new.path) {
        return None;
    }
    let mut lines = vec![format!("same role {}", quote(&old.role))];
    let place = landmark(&old.path);
    lines.push(if place.is_empty() {
        "no landmark on either side".to_string()
    } else {
        format!("same landmark {}", quote(place))
    });
    if let (Some(a), Some(b)) = (test_id(old), test_id(new)) {
        if a == b {
            lines.push(format!("same data-testid {}", quote(a)));
            return Some(Evidence {
                confidence: HealConfidence::High,
                lines,
            });
        }
    }
    let similarity = name_similarity(&old.name, &new.name);
    if similarity < NAME_THRESHOLD {
        return None;
    }
    let (dx, dy) = centre_move(old, new, run)?;
    if dx > MAX_MOVE || dy > MAX_MOVE {
        return None;
    }
    lines.push(format!(
        "name {} became {} (similarity {similarity:.2})",
        quote(&old.name),
        quote(&new.name)
    ));
    lines.push(format!(
        "box centre moved {:.1}% across and {:.1}% down the viewport",
        dx * 100.0,
        dy * 100.0
    ));
    Some(Evidence {
        confidence: HealConfidence::Medium,
        lines,
    })
}

fn quote(text: &str) -> String {
    format!("{:?}", traceqa_diff::text::plain(text))
}

/// `- old` and `+ new` lines for a change of locator, in Playwright TypeScript.
pub fn preview(old: &Locator, new: &Locator) -> String {
    format!("- {}\n+ {}", old.playwright_ts(), new.playwright_ts())
}

fn item(kind: HealKind, old: &Component, target: &str, action: HealAction) -> HealItem {
    HealItem {
        kind,
        id: old.id.clone(),
        new_id: None,
        target: Some(target.to_string()),
        role: Some(old.role.clone()),
        name: Some(old.name.clone()),
        action,
        confidence: None,
        evidence: Vec::new(),
        fix_preview: None,
        accepted: None,
    }
}

/// Classifies every component of `baseline` against `run`, one item each. Items of kind `unchanged`
/// are included; [`crate::build_report`] leaves them out of a report.
///
/// 1. A run that carries a `session_not_accepted` warning is signed out: it answers with that one
///    item and heals nothing, whatever else differs.
/// 2. The same id with another locator is a `locator_fix`.
/// 3. A removed id (one that an alias does not account for) with exactly one added component that
///    passes the evidence checks, which has no other removed rival, is a `rename_proposal`.
/// 4. With no such component it is `removed`; with several on either side it is `ambiguous`.
pub fn classify(
    baseline: &ComponentsFile,
    run: &ComponentsFile,
    aliases: &[BaselineAlias],
    target: &str,
) -> Result<Vec<HealItem>, HealError> {
    if run.warnings.iter().any(|w| w.code == SESSION_NOT_ACCEPTED) {
        return Ok(vec![HealItem {
            kind: HealKind::SessionNotAccepted,
            id: String::new(),
            new_id: None,
            target: Some(target.to_string()),
            role: None,
            name: None,
            action: HealAction::Refuse,
            confidence: None,
            evidence: vec![
                "the run's stored session was not accepted, so it shows a sign-in page and not the page that the baseline holds".to_string(),
                "nothing is healed: sign in again (traceqa login) and extract the page again".to_string(),
            ],
            fix_preview: None,
            accepted: None,
        }]);
    }
    let mut diff = compare(baseline, run, &Options::default()).map_err(|e| match e {
        DiffError::FingerprintVersion { a, b } => HealError::FingerprintVersion { baseline: a, run: b },
    })?;
    apply_aliases(&mut diff, aliases);
    let aliased: BTreeSet<&str> = diff
        .entries
        .iter()
        .filter(|e| e.kind == DiffEntryKind::Removed && e.paired_with.as_ref().is_some_and(|p| p.reason == "aliased"))
        .map(|e| e.id.as_str())
        .collect();

    let by_id: BTreeMap<&str, &Component> = run.components.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut items = Vec::new();
    let mut removed: Vec<&Component> = Vec::new();
    for old in &baseline.components {
        match by_id.get(old.id.as_str()) {
            Some(new) => items.push(same_id(old, new, target)),
            None if aliased.contains(old.id.as_str()) => {
                let mut unchanged = item(HealKind::Unchanged, old, target, HealAction::None);
                unchanged
                    .evidence
                    .push("a baseline alias accounts for the rename".to_string());
                items.push(unchanged);
            }
            None => removed.push(old),
        }
    }
    let baseline_ids: BTreeSet<&str> = baseline.components.iter().map(|c| c.id.as_str()).collect();
    let added: Vec<&Component> = run
        .components
        .iter()
        .filter(|c| !baseline_ids.contains(c.id.as_str()))
        .collect();

    // The evidence of every (removed, added) pair that passes the checks.
    let mut matrix: BTreeMap<(usize, usize), Evidence> = BTreeMap::new();
    for (r, old) in removed.iter().enumerate() {
        for (a, new) in added.iter().enumerate() {
            if let Some(evidence) = candidate(old, new, run) {
                matrix.insert((r, a), evidence);
            }
        }
    }
    for (r, old) in removed.iter().enumerate() {
        let mine: Vec<usize> = matrix.keys().filter(|(rr, _)| *rr == r).map(|&(_, a)| a).collect();
        let rivals = |a: usize| matrix.keys().filter(|(_, aa)| *aa == a).count();
        let heal = match mine[..] {
            [] => {
                let mut gone = item(HealKind::Removed, old, target, HealAction::Fail);
                gone.evidence
                    .push("the component is not in the run and nothing in the run is a safe match for it".to_string());
                gone
            }
            [a] if rivals(a) == 1 => {
                let new = added[a];
                let evidence = &matrix[&(r, a)];
                let mut proposal = item(HealKind::RenameProposal, old, target, HealAction::Propose);
                proposal.new_id = Some(new.id.clone());
                proposal.confidence = Some(evidence.confidence);
                proposal.evidence.clone_from(&evidence.lines);
                if let (Some(was), Some(now)) = (&old.locator, &new.locator) {
                    proposal.fix_preview = Some(preview(was, now));
                }
                proposal
            }
            _ => {
                let mut unclear = item(HealKind::Ambiguous, old, target, HealAction::Refuse);
                unclear.evidence.push(
                    "more than one component could be the match, on one side or the other, so none is proposed"
                        .to_string(),
                );
                for &a in &mine {
                    let others: Vec<&str> = matrix
                        .keys()
                        .filter(|&&(rr, aa)| aa == a && rr != r)
                        .map(|&(rr, _)| removed[rr].id.as_str())
                        .collect();
                    let also = if others.is_empty() {
                        String::new()
                    } else {
                        format!(", also a candidate for {}", others.join(", "))
                    };
                    unclear
                        .evidence
                        .push(format!("candidate {} {}{also}", added[a].id, quote(&added[a].name)));
                }
                unclear
            }
        };
        items.push(heal);
    }
    Ok(items)
}

/// A component both sides have: unchanged, or its locator drifted.
fn same_id(old: &Component, new: &Component, target: &str) -> HealItem {
    if let (Some(was), Some(now)) = (&old.locator, &new.locator) {
        if was.playwright_ts() != now.playwright_ts() {
            let mut fix = item(HealKind::LocatorFix, old, target, HealAction::Fix);
            fix.new_id = Some(new.id.clone());
            fix.fix_preview = Some(preview(was, now));
            fix.evidence
                .push("the id is the same, so it is the same component; only the way to find it changed".to_string());
            if now.forced {
                fix.evidence
                    .push("the new locator is a CSS selector, which breaks when the layout changes".to_string());
            } else if !now.unique {
                fix.evidence
                    .push("the new locator also matches other components".to_string());
            }
            return fix;
        }
    }
    item(HealKind::Unchanged, old, target, HealAction::None)
}
