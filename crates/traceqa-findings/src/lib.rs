//! Automated checks over `components.json`.
//!
//! Pure: it reads a [`ComponentsFile`] and returns a [`FindingsFile`]; it opens no file and
//! reads no clock (the caller passes today's date for suppression expiry). The engine and
//! `extract` do not use it. `traceqa findings` and, later, the MCP server and the SDKs compute
//! findings on demand.
//!
//! Every finding is an automated check, not a conformance claim.

mod cross_browser;
pub mod rules;
pub mod suppress;

use traceqa_protocol::types::{
    ComponentsFile, Finding, FindingCounts, FindingScope, FindingSeverity, FindingSuppression, FindingsFile, Warning,
};

pub use suppress::{SuppressionError, Suppressions};

/// Version of the `findings.json` layout.
pub const FORMAT_VERSION: u64 = 1;
/// Version of the rule set. Raise it when a rule is added or changes what it reports.
pub const RULES_VERSION: u64 = 1;

/// What to report.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Findings below this severity are left out. `None` keeps all.
    pub min_severity: Option<FindingSeverity>,
    /// Entries of the suppression file.
    pub suppressions: Option<Suppressions>,
    /// Today as `YYYY-MM-DD`. Without it no entry expires.
    pub today: Option<String>,
}

/// One target of a run.
pub struct TargetInput {
    /// Folder name inside the run folder.
    pub dir: String,
    pub file: ComponentsFile,
}

/// A whole run: every target that has a `components.json`.
pub struct RunInput {
    pub run_id: String,
    pub targets: Vec<TargetInput>,
}

/// Higher is worse.
pub fn severity_rank(severity: FindingSeverity) -> u8 {
    match severity {
        FindingSeverity::Info => 0,
        FindingSeverity::Warning => 1,
        FindingSeverity::Error => 2,
    }
}

/// Removes control characters and the bidirectional controls that reorder text, and shortens
/// to 200 characters. Page strings reach messages and evidence through this.
pub fn clean(text: &str) -> String {
    const BIDI: [char; 12] = [
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}',
        '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    let kept: String = text.chars().filter(|c| !c.is_control() && !BIDI.contains(c)).collect();
    if kept.chars().count() > 200 {
        let mut short: String = kept.chars().take(199).collect();
        short.push('…');
        short
    } else {
        kept
    }
}

/// Findings of one target. Warnings about suppression entries that matched nothing are about
/// this file alone; use [`analyse_run`] for a run, where an entry may match in another target.
pub fn analyse_target(file: &ComponentsFile, options: &Options) -> FindingsFile {
    let mut usage = suppress::Usage::default();
    let mut out = analyse_one(file, None, options, &mut usage);
    if let Some(suppressions) = &options.suppressions {
        out.warnings
            .extend(usage.warnings(suppressions, options.today.as_deref()));
    }
    out
}

/// Findings of every target, in the order of `run.targets`, then the rules that need the whole
/// run. A run-scope file is added last when there is something for it: a run-level finding or a
/// warning about the suppression file, which is judged across all targets.
pub fn analyse_run(run: &RunInput, options: &Options) -> Vec<FindingsFile> {
    let mut usage = suppress::Usage::default();
    let mut files: Vec<FindingsFile> = run
        .targets
        .iter()
        .map(|t| analyse_one(&t.file, Some(t.dir.clone()), options, &mut usage))
        .collect();

    let mut run_level = rules::run_findings(run);
    sort(&mut run_level);
    let (kept, suppressed) = apply_suppressions(run_level, options, &mut usage);
    let warnings = options
        .suppressions
        .as_ref()
        .map(|s| usage.warnings(s, options.today.as_deref()))
        .unwrap_or_default();
    if !kept.is_empty() || !suppressed.is_empty() || !warnings.is_empty() {
        files.push(assemble(
            FindingScope::Run,
            &run.run_id,
            None,
            None,
            kept,
            suppressed,
            warnings,
            options,
        ));
    }
    files
}

fn analyse_one(
    file: &ComponentsFile,
    target: Option<String>,
    options: &Options,
    usage: &mut suppress::Usage,
) -> FindingsFile {
    let mut found = rules::target_findings(file);
    sort(&mut found);
    let (kept, suppressed) = apply_suppressions(found, options, usage);
    let mut warnings = Vec::new();
    let has_names = file
        .components
        .iter()
        .flat_map(|c| &c.instances)
        .any(|i| !i.name.is_empty());
    let has_sources = file
        .components
        .iter()
        .flat_map(|c| &c.instances)
        .any(|i| i.name_source.is_some());
    if has_names && !has_sources {
        warnings.push(Warning {
            code: "name_source_missing".into(),
            message: "This components.json was written before instances recorded where their names came from, so the weak-name-source rule could not run. Extract the page again.".into(),
        });
    }
    assemble(
        FindingScope::Target,
        &file.run_id,
        target,
        Some(file),
        kept,
        suppressed,
        warnings,
        options,
    )
}

fn sort(findings: &mut [Finding]) {
    let order = |rule: &str| rules::RULES.iter().position(|r| *r == rule).unwrap_or(usize::MAX);
    findings.sort_by(|a, b| {
        severity_rank(b.severity)
            .cmp(&severity_rank(a.severity))
            .then(order(&a.rule).cmp(&order(&b.rule)))
            .then(a.component_id.cmp(&b.component_id))
    });
}

fn apply_suppressions(
    findings: Vec<Finding>,
    options: &Options,
    usage: &mut suppress::Usage,
) -> (Vec<Finding>, Vec<Finding>) {
    let Some(suppressions) = &options.suppressions else {
        return (findings, Vec::new());
    };
    let mut kept = Vec::new();
    let mut suppressed = Vec::new();
    for mut finding in findings {
        match suppressions.find(&finding.id, options.today.as_deref()) {
            Some(index) => {
                usage.mark(index);
                let entry = &suppressions.entries[index];
                finding.suppressed = Some(FindingSuppression {
                    reason: entry.reason.clone(),
                    expires: entry.expires.clone(),
                });
                suppressed.push(finding);
            }
            None => kept.push(finding),
        }
    }
    (kept, suppressed)
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    scope: FindingScope,
    run_id: &str,
    target: Option<String>,
    file: Option<&ComponentsFile>,
    mut findings: Vec<Finding>,
    mut suppressed: Vec<Finding>,
    warnings: Vec<Warning>,
    options: &Options,
) -> FindingsFile {
    if let Some(min) = options.min_severity {
        let keep = |f: &Finding| severity_rank(f.severity) >= severity_rank(min);
        findings.retain(keep);
        suppressed.retain(keep);
    }
    let count = |severity: FindingSeverity| findings.iter().filter(|f| f.severity == severity).count() as u64;
    let counts = FindingCounts {
        error: count(FindingSeverity::Error),
        warning: count(FindingSeverity::Warning),
        info: count(FindingSeverity::Info),
    };
    FindingsFile {
        format_version: FORMAT_VERSION,
        rules_version: RULES_VERSION,
        scope,
        run_id: run_id.to_string(),
        target,
        url: file.map(|f| f.url.clone()),
        browser: file.map(|f| f.browser.clone()),
        viewport: file.map(|f| f.viewport.clone()),
        counts,
        findings,
        suppressed,
        warnings,
    }
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // test builders take JSON by value to read like the data they make
mod tests {
    use super::*;
    use serde_json::json;

    fn file_with(components: serde_json::Value) -> ComponentsFile {
        serde_json::from_value(json!({
            "formatVersion": 1, "fingerprintVersion": 2, "runId": "r_1", "url": "https://x.test",
            "finalUrl": "https://x.test/", "title": "t", "browser": { "name": "chromium", "version": "1" },
            "viewport": { "width": 1280, "height": 800 }, "capturedAt": "2026-01-01T00:00:00Z",
            "components": components, "warnings": []
        }))
        .unwrap()
    }

    fn unnamed(id: &str) -> serde_json::Value {
        json!({
            "id": id, "role": "button", "name": "", "nameIsTemplate": false, "confidence": "high",
            "signals": ["native"], "path": "main", "instances": [{
                "key": format!("{id}:0"), "name": "", "tag": "button", "attrs": {}, "selectors": [],
                "box": { "x": 0, "y": 0, "w": 40, "h": 40 },
                "state": { "visible": true, "enabled": true, "inViewport": true },
                "frameId": "main", "inShadowDom": false
            }]
        })
    }

    fn with(text: &str, today: Option<&str>) -> Options {
        Options {
            suppressions: Some(suppress::parse(text).unwrap()),
            today: today.map(str::to_string),
            ..Options::default()
        }
    }

    #[test]
    fn a_suppression_matches_by_id_and_keeps_the_reason() {
        let f = file_with(json!([unnamed("c_000000000001")]));
        let out = analyse_target(
            &f,
            &with(
                "suppressions:\n  - id: unnamed-control:c_000000000001\n    reason: icon font\n",
                None,
            ),
        );
        assert!(out.findings.is_empty());
        assert_eq!(out.suppressed[0].suppressed.as_ref().unwrap().reason, "icon font");
        assert_eq!(out.counts.error, 0);
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn an_expired_entry_is_ignored_with_a_warning() {
        let f = file_with(json!([unnamed("c_000000000001")]));
        let text =
            "suppressions:\n  - id: unnamed-control:c_000000000001\n    reason: later\n    expires: 2026-03-01\n";
        let out = analyse_target(&f, &with(text, Some("2026-03-02")));
        assert_eq!(out.findings.len(), 1);
        assert!(out.suppressed.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].code, "suppression_expired");
        // On the last day it still applies.
        let still = analyse_target(&f, &with(text, Some("2026-03-01")));
        assert!(still.findings.is_empty());
    }

    #[test]
    fn an_entry_that_matches_nothing_is_warned_about_but_does_not_fail() {
        let f = file_with(json!([unnamed("c_000000000001")]));
        let out = analyse_target(
            &f,
            &with(
                "suppressions:\n  - id: small-target:c_ffffffffffff\n    reason: old\n",
                None,
            ),
        );
        assert_eq!(out.findings.len(), 1);
        assert_eq!(out.warnings[0].code, "suppression_unused");
    }

    #[test]
    fn in_a_run_an_entry_used_by_any_target_is_not_unused() {
        let run = RunInput {
            run_id: "r_1".into(),
            targets: vec![
                TargetInput {
                    dir: "chromium-1280x800".into(),
                    file: file_with(json!([unnamed("c_000000000001")])),
                },
                TargetInput {
                    dir: "firefox-1280x800".into(),
                    // The same components in both browsers: no browsers-disagree finding.
                    file: file_with(json!([unnamed("c_000000000001")])),
                },
            ],
        };
        let options = with(
            "suppressions:\n  - id: unnamed-control:c_000000000001\n    reason: ok\n",
            None,
        );
        let files = analyse_run(&run, &options);
        assert_eq!(files.len(), 2, "no run-scope file without something to say");
        assert_eq!(files[0].target.as_deref(), Some("chromium-1280x800"));
        assert_eq!(files[0].suppressed.len(), 1);

        let unused = with(
            "suppressions:\n  - id: unnamed-control:c_999999999999\n    reason: ok\n",
            None,
        );
        let files = analyse_run(&run, &unused);
        assert_eq!(files.len(), 3);
        assert_eq!(files[2].scope, FindingScope::Run);
        assert_eq!(files[2].warnings[0].code, "suppression_unused");
    }

    #[test]
    fn min_severity_filters_findings_and_counts() {
        let f = file_with(json!([unnamed("c_000000000001")]));
        let out = analyse_target(
            &f,
            &Options {
                min_severity: Some(FindingSeverity::Error),
                ..Options::default()
            },
        );
        assert_eq!(out.counts.error, 1);
        let none = analyse_target(&f, &Options::default());
        assert_eq!(none.findings.len(), 1);
        let mut warn_only = file_with(json!([unnamed("c_000000000001")]));
        warn_only.components[0].confidence = traceqa_protocol::types::Confidence::Low;
        let out = analyse_target(
            &warn_only,
            &Options {
                min_severity: Some(FindingSeverity::Error),
                ..Options::default()
            },
        );
        assert!(out.findings.is_empty());
    }

    #[test]
    fn output_is_deterministic_and_sorted_worst_first() {
        let f = file_with(json!([unnamed("c_000000000002"), unnamed("c_000000000001")]));
        let a = serde_json::to_string(&analyse_target(&f, &Options::default())).unwrap();
        let b = serde_json::to_string(&analyse_target(&f, &Options::default())).unwrap();
        assert_eq!(a, b);
        let out = analyse_target(&f, &Options::default());
        assert_eq!(out.findings[0].component_id, "c_000000000001");
    }

    #[test]
    fn old_files_without_name_sources_get_one_warning() {
        let mut f = file_with(json!([unnamed("c_000000000001")]));
        f.components[0].instances[0].name = "Go".into();
        let out = analyse_target(&f, &Options::default());
        assert_eq!(out.warnings[0].code, "name_source_missing");
    }

    /// The documented example is what the rules really produce for the documented input.
    #[test]
    fn the_spec_example_is_what_the_rules_produce() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples");
        let input: ComponentsFile =
            serde_json::from_str(&std::fs::read_to_string(dir.join("ComponentsFile.with-name-source.json")).unwrap())
                .unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("FindingsFile.target.json")).unwrap()).unwrap();
        let mut out = analyse_target(&input, &Options::default());
        out.target = Some("chromium-1280x800".into());
        // The example leaves out helpUrl: examples may only name reserved example domains.
        for finding in &mut out.findings {
            assert!(finding
                .help_url
                .as_deref()
                .is_some_and(|u| u.starts_with("https://www.w3.org/WAI/WCAG22/")));
            finding.help_url = None;
        }
        assert_eq!(serde_json::to_value(&out).unwrap(), expected);
    }

    #[test]
    fn clean_removes_controls_and_bidi() {
        assert_eq!(clean("a\u{202e}b\u{0}c\nd"), "abcd");
        assert_eq!(clean(&"x".repeat(300)).chars().count(), 200);
    }
}
