//! Comparing two `components.json` files.

use std::collections::BTreeMap;
use std::fmt;

use traceqa_protocol::types::{
    Component, ComponentSummary, ComponentsFile, DiffEntry, DiffEntryKind, DiffFile, DiffSide, DiffSummary, PairLink,
    Warning,
};

use crate::pair::{self, Candidate};

/// `DiffFile.formatVersion`.
pub const FORMAT_VERSION: u64 = 1;

/// What the caller knows about the two sides and the files do not.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// What the person typed for side A, shown in the report.
    pub label_a: String,
    pub label_b: String,
    /// The target folder names inside their runs, when known.
    pub target_a: Option<String>,
    pub target_b: Option<String>,
}

/// Why two files cannot be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    /// Ids made by different versions of the recipe must not be compared (`spec/fingerprint.md`).
    FingerprintVersion { a: u64, b: u64 },
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FingerprintVersion { a, b } => write!(
                f,
                "the two sides use different fingerprint versions ({a} and {b}), so their component ids cannot \
                 be compared; extract both again with the same TraceQA version"
            ),
        }
    }
}

impl std::error::Error for DiffError {}

/// The names of the fields [`changed_fields`] can report, in the order it reports them.
pub const FIELDS: [&str; 9] = [
    "instances",
    "visible",
    "enabled",
    "covered",
    "checked",
    "expanded",
    "selected",
    "pressed",
    "confidence",
];

/// What a diff compares about one component: its identity as shown, and how many instances are in
/// each state. Boxes, crops, selectors and attributes are left out on purpose: they move with
/// every layout change and would bury the differences that matter.
pub fn summarise(component: &Component) -> ComponentSummary {
    let count = |test: &dyn Fn(&traceqa_protocol::types::ElementState) -> bool| {
        component.instances.iter().filter(|i| test(&i.state)).count() as u64
    };
    ComponentSummary {
        role: component.role.clone(),
        name: component.name.clone(),
        name_is_template: component.name_is_template,
        path: component.path.clone(),
        instances: component.instances.len() as u64,
        visible: count(&|s| s.visible),
        enabled: count(&|s| s.enabled),
        covered: count(&|s| s.covered == Some(true)),
        checked: count(&|s| s.checked == Some(true)),
        expanded: count(&|s| s.expanded == Some(true)),
        selected: count(&|s| s.selected == Some(true)),
        pressed: count(&|s| s.pressed == Some(true)),
        confidence: component.confidence,
    }
}

/// The [`FIELDS`] in which two summaries of one id differ.
pub fn changed_fields(a: &ComponentSummary, b: &ComponentSummary) -> Vec<String> {
    let differs = [
        a.instances != b.instances,
        a.visible != b.visible,
        a.enabled != b.enabled,
        a.covered != b.covered,
        a.checked != b.checked,
        a.expanded != b.expanded,
        a.selected != b.selected,
        a.pressed != b.pressed,
        a.confidence != b.confidence,
    ];
    FIELDS
        .iter()
        .zip(differs)
        .filter(|(_, differs)| *differs)
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// The gating facts of a change, in a fixed order. See `DiffEntry.flags`.
pub fn flags(a: &ComponentSummary, b: &ComponentSummary) -> Vec<String> {
    let tests = [
        ("became_hidden", a.visible > 0 && b.visible == 0),
        ("became_visible", a.visible == 0 && b.visible > 0),
        ("became_disabled", a.enabled > 0 && b.enabled == 0),
        ("became_enabled", a.enabled == 0 && b.enabled > 0),
        ("became_covered", a.covered == 0 && b.covered > 0),
        ("became_uncovered", a.covered > 0 && b.covered == 0),
        ("count_up", b.instances > a.instances),
        ("count_down", b.instances < a.instances),
    ];
    tests
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name.to_string())
        .collect()
}

fn side(file: &ComponentsFile, label: &str, target: Option<&String>) -> DiffSide {
    DiffSide {
        label: label.to_string(),
        run_id: file.run_id.clone(),
        target: target.cloned(),
        browser: file.browser.clone(),
        viewport: file.viewport.clone(),
        url: file.url.clone(),
        final_url: file.final_url.clone(),
        captured_at: file.captured_at.clone(),
        fingerprint_version: file.fingerprint_version,
        component_count: file.components.len() as u64,
    }
}

/// What kind of comparison this is.
fn mode(a: &ComponentsFile, b: &ComponentsFile) -> &'static str {
    let same_run = a.run_id == b.run_id;
    let same_browser = a.browser.name == b.browser.name;
    let same_viewport = a.viewport == b.viewport;
    match (same_run, same_browser, same_viewport) {
        (true, false, true) => "cross-browser",
        (true, true, false) => "cross-viewport",
        (false, true, true) => "run-to-run",
        _ => "custom",
    }
}

fn warn(code: &str, message: String) -> Warning {
    Warning {
        code: code.to_string(),
        message,
    }
}

/// Compares two files by component id. B is the newer or the other side: `added` means in B only.
///
/// Fails when the files use different fingerprint versions. The result depends on nothing but the
/// arguments, and its entries are ordered by kind (removed, added, changed), then path, role,
/// name and id, whatever the order of the components in the files.
pub fn compare(a: &ComponentsFile, b: &ComponentsFile, options: &Options) -> Result<DiffFile, DiffError> {
    if a.fingerprint_version != b.fingerprint_version {
        return Err(DiffError::FingerprintVersion {
            a: a.fingerprint_version,
            b: b.fingerprint_version,
        });
    }
    let in_a: BTreeMap<&str, ComponentSummary> = a.components.iter().map(|c| (c.id.as_str(), summarise(c))).collect();
    let in_b: BTreeMap<&str, ComponentSummary> = b.components.iter().map(|c| (c.id.as_str(), summarise(c))).collect();

    let mut removed: Vec<(&str, &ComponentSummary)> = Vec::new();
    let mut added: Vec<(&str, &ComponentSummary)> = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = 0u64;
    for (&id, summary_a) in &in_a {
        match in_b.get(id) {
            None => removed.push((id, summary_a)),
            Some(summary_b) => {
                let fields = changed_fields(summary_a, summary_b);
                if fields.is_empty() {
                    unchanged += 1;
                } else {
                    changed.push(DiffEntry {
                        kind: DiffEntryKind::Changed,
                        id: id.to_string(),
                        a: Some(summary_a.clone()),
                        b: Some(summary_b.clone()),
                        fields,
                        flags: flags(summary_a, summary_b),
                        paired_with: None,
                    });
                }
            }
        }
    }
    for (&id, summary_b) in &in_b {
        if !in_a.contains_key(id) {
            added.push((id, summary_b));
        }
    }
    let order = |x: &(&str, &ComponentSummary), y: &(&str, &ComponentSummary)| {
        let key =
            |e: &(&str, &ComponentSummary)| (e.1.path.clone(), e.1.role.clone(), e.1.name.clone(), e.0.to_string());
        key(x).cmp(&key(y))
    };
    removed.sort_by(order);
    added.sort_by(order);
    changed.sort_by(|x, y| {
        let key = |e: &DiffEntry| {
            let s = e.b.as_ref().or(e.a.as_ref());
            (
                s.map(|s| s.path.clone()),
                s.map(|s| s.role.clone()),
                s.map(|s| s.name.clone()),
                e.id.clone(),
            )
        };
        key(x).cmp(&key(y))
    });

    let removed_candidates: Vec<Candidate> = removed.iter().map(|&(id, summary)| Candidate { id, summary }).collect();
    let added_candidates: Vec<Candidate> = added.iter().map(|&(id, summary)| Candidate { id, summary }).collect();
    let pairs = pair::pair(&removed_candidates, &added_candidates);
    let link = |id: &str, p: &pair::Pair| PairLink {
        id: id.to_string(),
        reason: p.reason.to_string(),
        confidence: p.confidence,
    };
    let mut removed_links: BTreeMap<usize, PairLink> = BTreeMap::new();
    let mut added_links: BTreeMap<usize, PairLink> = BTreeMap::new();
    for p in &pairs {
        removed_links.insert(p.removed, link(added[p.added].0, p));
        added_links.insert(p.added, link(removed[p.removed].0, p));
    }

    let mut entries = Vec::with_capacity(removed.len() + added.len() + changed.len());
    for (i, &(id, summary)) in removed.iter().enumerate() {
        entries.push(DiffEntry {
            kind: DiffEntryKind::Removed,
            id: id.to_string(),
            a: Some(summary.clone()),
            b: None,
            fields: Vec::new(),
            flags: Vec::new(),
            paired_with: removed_links.remove(&i),
        });
    }
    for (i, &(id, summary)) in added.iter().enumerate() {
        entries.push(DiffEntry {
            kind: DiffEntryKind::Added,
            id: id.to_string(),
            a: None,
            b: Some(summary.clone()),
            fields: Vec::new(),
            flags: Vec::new(),
            paired_with: added_links.remove(&i),
        });
    }
    let summary = DiffSummary {
        added: added.len() as u64,
        removed: removed.len() as u64,
        changed: changed.len() as u64,
        unchanged,
        paired: pairs.len() as u64,
    };
    entries.extend(changed);

    let mut warnings = Vec::new();
    if a.viewport != b.viewport {
        warnings.push(warn(
            "viewports_differ",
            "The two sides were read at different viewports. Components that a site hides on purpose at one size \
             are expected to differ."
                .to_string(),
        ));
    }
    let shared = summary.changed + summary.unchanged;
    let smaller = a.components.len().min(b.components.len()) as u64;
    if smaller > 0 && shared * 2 < smaller {
        warnings.push(warn(
            "few_shared_components",
            format!(
                "Only {shared} components are on both sides, fewer than half of the smaller side's {smaller}. The \
                 pages may differ in more than state, for example when a site serves another page at another size."
            ),
        ));
    }

    Ok(DiffFile {
        format_version: FORMAT_VERSION,
        mode: mode(a, b).to_string(),
        a: side(a, &options.label_a, options.target_a.as_ref()),
        b: side(b, &options.label_b, options.target_b.as_ref()),
        summary,
        entries,
        warnings,
    })
}
