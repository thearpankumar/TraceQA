//! SARIF 2.1.0 output for a diff or for findings.
//!
//! A web component has no source file, so a result has no region: its location is the page address
//! (`artifactLocation.uri`) and a logical location that names the component
//! (`<address>#<target>/<id>`). GitHub code scanning wants repo-relative source files with regions,
//! so this file is for SARIF viewers and other tools; `--format github` is the CI-native path.
//!
//! Rule ids are stable (`traceqa.diff.removed`, `traceqa.findings.unnamed-control`), and a result's
//! `partialFingerprints` carry a versioned name (`traceqaComponent/v1`) as SARIF 3.5.4.2 asks.
//! Page text is cleaned and cut before it reaches a message.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use traceqa_common::types::{DiffEntryKind, DiffFile, FindingSeverity, FindingsFile};

use crate::gate::{classify, reportable, Class, Severity};
use crate::github::entry_message;
use crate::text::plain;

const SCHEMA: &str =
    "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json";

struct Rule {
    id: String,
    short: String,
    full: String,
    help: String,
    help_uri: Option<String>,
    level: &'static str,
}

struct Item {
    rule_id: String,
    level: &'static str,
    message: String,
    uri: String,
    logical_name: String,
    qualified_name: String,
    fingerprint: Option<(&'static str, String)>,
    baseline_state: Option<&'static str>,
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

fn diff_rule(class: Option<Class>) -> Rule {
    let (name, short, full, severity) = match class {
        Some(Class::Removed) => (
            "removed",
            "A component of the baseline is gone",
            "The component is in the baseline but the run has no component with its id. Its role, name or place on the page changed, or it was removed.",
            Severity::Error,
        ),
        Some(Class::Added) => (
            "added",
            "A component is new",
            "The run has a component that the baseline does not have.",
            Severity::Warning,
        ),
        Some(Class::Disabled) => (
            "disabled",
            "A component became disabled",
            "Every instance that was enabled in the baseline is disabled in the run.",
            Severity::Error,
        ),
        Some(Class::Hidden) => (
            "hidden",
            "A component became hidden",
            "Every instance that was visible in the baseline is hidden in the run.",
            Severity::Error,
        ),
        Some(Class::Covered) => (
            "covered",
            "A component became covered",
            "An instance is covered by another element in the run and was not in the baseline.",
            Severity::Error,
        ),
        Some(Class::Changed) => (
            "changed",
            "A component changed",
            "The component is on both sides and differs in instance count, state or confidence.",
            Severity::Warning,
        ),
        Some(Class::Renamed) => (
            "renamed",
            "A component was renamed",
            "A baseline alias says the removed component and the added one are the same component.",
            Severity::Note,
        ),
        None => (
            "target-missing",
            "A baseline target is missing from the run",
            "The baseline has a browser-and-viewport folder that the run does not have, so it could not be compared.",
            Severity::Error,
        ),
    };
    Rule {
        id: format!("traceqa.diff.{name}"),
        short: short.into(),
        full: full.into(),
        help: "Open the baseline and the run in a TraceQA diff. If the change is intended, run `traceqa baseline promote RUN --yes`; if a component was only renamed, run `traceqa baseline alias`.".to_string(),
        help_uri: None,
        level: level(severity),
    }
}

const ALL_CLASSES: [Class; 7] = [
    Class::Removed,
    Class::Added,
    Class::Changed,
    Class::Disabled,
    Class::Hidden,
    Class::Covered,
    Class::Renamed,
];

fn render(rules: &[Rule], items: &[Item]) -> Value {
    let rule_json: Vec<Value> = rules
        .iter()
        .map(|r| {
            let mut v = json!({
                "id": r.id,
                "shortDescription": { "text": r.short },
                "fullDescription": { "text": r.full },
                "help": { "text": r.help },
                "defaultConfiguration": { "level": r.level },
            });
            if let Some(uri) = &r.help_uri {
                v["helpUri"] = json!(uri);
            }
            v
        })
        .collect();
    let results: Vec<Value> = items
        .iter()
        .map(|i| {
            let mut v = json!({
                "ruleId": i.rule_id,
                "ruleIndex": rules.iter().position(|r| r.id == i.rule_id),
                "level": i.level,
                "message": { "text": i.message },
                "locations": [{
                    "physicalLocation": { "artifactLocation": { "uri": i.uri } },
                    "logicalLocations": [{
                        "name": i.logical_name,
                        "fullyQualifiedName": i.qualified_name,
                        "kind": "element",
                    }],
                }],
            });
            if let Some((name, value)) = &i.fingerprint {
                let mut prints = serde_json::Map::new();
                prints.insert((*name).to_string(), json!(value));
                v["partialFingerprints"] = Value::Object(prints);
            }
            if let Some(state) = i.baseline_state {
                v["baselineState"] = json!(state);
            }
            v
        })
        .collect();
    json!({
        "$schema": SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "traceqa",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": env!("CARGO_PKG_REPOSITORY"),
                "rules": rule_json,
            }},
            "results": results,
        }],
    })
}

/// The SARIF log of the comparison of a run with a baseline. `fallback_url` is the page address
/// used for a target that has no diff. Each entry is one result; the added half of an aliased
/// pair is left out, because the removed half already says it was renamed.
pub fn diff_log(fallback_url: &str, outcomes: &[(&str, Result<&DiffFile, &str>)]) -> Value {
    let rules: Vec<Rule> = ALL_CLASSES
        .iter()
        .map(|c| diff_rule(Some(*c)))
        .chain(std::iter::once(diff_rule(None)))
        .collect();
    let mut items = Vec::new();
    for (target, outcome) in outcomes {
        match outcome {
            Err(why) => items.push(Item {
                rule_id: "traceqa.diff.target-missing".into(),
                level: "error",
                message: format!("{}: {}", plain(target), plain(why)),
                uri: fallback_url.to_string(),
                logical_name: (*target).to_string(),
                qualified_name: format!("{fallback_url}#{target}"),
                fingerprint: Some(("traceqaComponent/v1", format!("{target}/target-missing"))),
                baseline_state: None,
            }),
            Ok(diff) => {
                for entry in reportable(diff) {
                    let class = classify(entry);
                    items.push(Item {
                        rule_id: format!("traceqa.diff.{}", class.name()),
                        level: level(class.severity()),
                        message: entry_message(target, entry),
                        uri: diff.b.url.clone(),
                        logical_name: entry.id.clone(),
                        qualified_name: format!("{}#{}/{}", diff.b.url, target, entry.id),
                        fingerprint: Some(("traceqaComponent/v1", format!("{target}/{}:{}", entry.id, class.name()))),
                        baseline_state: match entry.kind {
                            DiffEntryKind::Added => Some("new"),
                            DiffEntryKind::Removed => Some("absent"),
                            DiffEntryKind::Changed => None,
                        },
                    });
                }
            }
        }
    }
    render(&rules, &items)
}

fn finding_rule_text(rule: &str) -> (&'static str, &'static str) {
    match rule {
        "unnamed-control" => (
            "A control has no accessible name",
            "A button, link or form control has no name that a screen reader could announce.",
        ),
        "weak-name-source" => (
            "A control is named only by its title or placeholder",
            "The name comes from a source that assistive technology treats as a fallback.",
        ),
        "fake-button" => (
            "A clickable element has no role",
            "An element looks and acts like a button but has no button role, so a keyboard or screen reader user cannot find it.",
        ),
        "small-target" => (
            "A target is smaller than 24 by 24 CSS pixels",
            "WCAG 2.5.8 (Target Size, Minimum) asks for 24 by 24 CSS pixels unless an exception applies.",
        ),
        "covered-at-load" => (
            "A control is covered when the page loads",
            "Another element sits on top of the control, so a pointer click lands on that element instead.",
        ),
        "ambiguous-names" => (
            "Several controls share a role and name",
            "A person using a screen reader cannot tell them apart.",
        ),
        "missing-autocomplete" => (
            "A field about the person has no autocomplete token",
            "WCAG 1.3.5 (Identify Input Purpose) asks for an autocomplete attribute on fields that collect user data.",
        ),
        _ => ("An automated check found a problem", "See the message of the result."),
    }
}

/// The SARIF log of findings. Rules are the ones that produced a result, in order of first use.
pub fn findings_log(files: &[&FindingsFile]) -> Value {
    let mut rules: Vec<Rule> = Vec::new();
    let mut items = Vec::new();
    let mut by_rule: BTreeMap<String, ()> = BTreeMap::new();
    for file in files {
        let target = file.target.as_deref().unwrap_or("run");
        let url = file.url.clone().unwrap_or_default();
        for f in &file.findings {
            let id = format!("traceqa.findings.{}", f.rule);
            let lvl = match f.severity {
                FindingSeverity::Error => "error",
                FindingSeverity::Warning => "warning",
                FindingSeverity::Info => "note",
            };
            if by_rule.insert(id.clone(), ()).is_none() {
                let (short, full) = finding_rule_text(&f.rule);
                rules.push(Rule {
                    id: id.clone(),
                    short: short.into(),
                    full: format!("{full} This is an automated check, not a conformance claim."),
                    help: "See docs/usage.md (Findings) for what the rule measured and how to suppress it.".into(),
                    help_uri: f.help_url.clone().filter(|u| u.starts_with("https://")),
                    level: lvl,
                });
            }
            items.push(Item {
                rule_id: id,
                level: lvl,
                message: format!(
                    "{}: {} \"{}\": {}",
                    plain(target),
                    plain(&f.role),
                    plain(&f.name),
                    plain(&f.message)
                ),
                uri: url.clone(),
                logical_name: f.component_id.clone(),
                qualified_name: format!("{url}#{target}/{}", f.component_id),
                fingerprint: Some(("traceqaFinding/v1", format!("{target}/{}", f.id))),
                baseline_state: None,
            });
        }
    }
    render(&rules, &items)
}
