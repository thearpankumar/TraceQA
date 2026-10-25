//! Findings from axe-core's results (`axe.json`), one per rule and component.
//!
//! The results were collected and placed on components by the engine when the run was made with
//! `--axe`. This module only turns them into findings, like the other rules, and keeps the
//! limits of automated checking in front of the reader.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use traceqa_protocol::types::{
    AxeNode, AxeResults, AxeViolation, Component, ComponentsFile, Finding, FindingConfidence, FindingSeverity, Rect,
};

use crate::{clean, rules};

/// Said in every findings file that includes axe-core results.
pub const COVERAGE: &str = "Automated rules find on average about 57 percent of WCAG issues (Deque); this is not a conformance claim. \
Contrast over images and gradients, meaning of text and alt text, keyboard order, focus visibility, hidden or closed regions \
(menus, dialogs), time limits and the quality of captions need a person. Results axe could not decide are counted as needing review.";

/// What a finding says on top of axe's own help text.
const NOT_A_CLAIM: &str = "This is an automated check, not a conformance claim.";
const MAX_INSTANCES: usize = 20;
const MAX_EVIDENCE_NODES: usize = 5;

/// The component id used when no component contains the element.
pub const PAGE: &str = "page";

/// axe's impact as a severity. Without an impact the finding is only worth a look.
pub fn severity(impact: Option<&str>) -> FindingSeverity {
    match impact {
        Some("critical" | "serious") => FindingSeverity::Error,
        Some("moderate") => FindingSeverity::Warning,
        _ => FindingSeverity::Info,
    }
}

/// WCAG success criteria in axe's tags: `wcag143` is 1.4.3. The level tags (`wcag2aa`,
/// `wcag22aa`) carry letters and name no criterion.
pub fn wcag_numbers(tags: &[String]) -> Vec<String> {
    let mut numbers: Vec<String> = tags
        .iter()
        .filter_map(|tag| {
            let digits = tag.strip_prefix("wcag")?;
            if !(3..=5).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let (principle, rest) = digits.split_at(1);
            let (guideline, criterion) = rest.split_at(1);
            Some(format!("{principle}.{guideline}.{criterion}"))
        })
        .collect();
    numbers.sort();
    numbers.dedup();
    numbers
}

/// One finding per failed rule and component. Elements no component contains make one page-level
/// finding per rule, whose component id is `page`.
pub(crate) fn findings(results: &AxeResults, file: &ComponentsFile) -> Vec<Finding> {
    let mut out = Vec::new();
    for violation in &results.violations {
        let mut by_component: BTreeMap<&str, Vec<&AxeNode>> = BTreeMap::new();
        for node in &violation.nodes {
            by_component
                .entry(node.component_id.as_deref().unwrap_or(PAGE))
                .or_default()
                .push(node);
        }
        for (component_id, nodes) in by_component {
            out.push(one(results, violation, component_id, &nodes, file));
        }
    }
    out
}

fn one(
    results: &AxeResults,
    violation: &AxeViolation,
    component_id: &str,
    nodes: &[&AxeNode],
    file: &ComponentsFile,
) -> Finding {
    let component = file.components.iter().find(|c| c.id == component_id);
    let rule = format!("axe.{}", clean(&violation.rule));
    let count = nodes.len();
    let message = format!(
        "{} ({count} element{} on {}.) {NOT_A_CLAIM}",
        clean(&violation.help),
        if count == 1 { "" } else { "s" },
        component.map_or("the page, outside any component", |_| "this component"),
    );
    let seen: Vec<_> = nodes
        .iter()
        .take(MAX_EVIDENCE_NODES)
        .map(|node| json!({ "target": node.target, "frameId": node.frame_id, "viaAncestor": node.via_ancestor.unwrap_or(false) }))
        .collect();
    let evidence = json!({
        "source": "axe-core",
        "axeVersion": results.axe_version,
        "impact": violation.impact,
        "elements": count,
        "nodes": seen,
    });
    // Page text (selectors) reaches evidence, so it is cleaned like every other rule's.
    let Value::Object(evidence) = evidence else {
        unreachable!("evidence is always built as an object")
    };
    let evidence = rules::sanitise_map(evidence);
    let (role, name, path) = match component {
        Some(c) => (clean(&c.role), clean(&c.name), clean(&c.path)),
        None => ("page".to_string(), clean(&file.title), "page".to_string()),
    };
    Finding {
        id: format!("{rule}:{component_id}"),
        rule,
        severity: severity(violation.impact.as_deref()),
        confidence: FindingConfidence::Medium,
        component_id: component_id.to_string(),
        instances: component.map(|c| instance_keys(c, nodes)).unwrap_or_default(),
        role,
        name,
        path,
        message,
        evidence,
        wcag: wcag_numbers(&violation.tags),
        help_url: Some(violation.help_url.clone()),
        suppressed: None,
    }
}

/// The instances of `component` the failing elements are in: the smallest one containing each
/// element's centre, in its frame.
fn instance_keys(component: &Component, nodes: &[&AxeNode]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for node in nodes {
        let smallest = component
            .instances
            .iter()
            .filter(|i| i.frame_id == node.frame_id && contains_centre(&i.box_, &node.box_))
            .min_by(|a, b| (a.box_.w * a.box_.h).total_cmp(&(b.box_.w * b.box_.h)));
        if let Some(instance) = smallest {
            if !keys.contains(&instance.key) {
                keys.push(instance.key.clone());
            }
        }
        if keys.len() >= MAX_INSTANCES {
            break;
        }
    }
    keys
}

fn contains_centre(outer: &Rect, inner: &Rect) -> bool {
    let (cx, cy) = (inner.x + inner.w / 2.0, inner.y + inner.h / 2.0);
    cx >= outer.x && cx <= outer.x + outer.w && cy >= outer.y && cy <= outer.y + outer.h
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // test builders take JSON by value to read like the data they make
mod tests {
    use super::*;

    #[test]
    fn impact_maps_to_severity() {
        assert_eq!(severity(Some("critical")), FindingSeverity::Error);
        assert_eq!(severity(Some("serious")), FindingSeverity::Error);
        assert_eq!(severity(Some("moderate")), FindingSeverity::Warning);
        assert_eq!(severity(Some("minor")), FindingSeverity::Info);
        assert_eq!(severity(None), FindingSeverity::Info);
        assert_eq!(severity(Some("something-new")), FindingSeverity::Info);
    }

    #[test]
    fn wcag_numbers_come_from_criterion_tags_only() {
        let tags: Vec<String> = [
            "cat.color",
            "wcag2aa",
            "wcag143",
            "wcag22aa",
            "wcag1410",
            "wcag2411",
            "best-practice",
            "wcag143",
        ]
        .iter()
        .map(|t| (*t).to_string())
        .collect();
        assert_eq!(wcag_numbers(&tags), ["1.4.10", "1.4.3", "2.4.11"]);
        assert!(wcag_numbers(&["wcag".to_string(), "wcag12".to_string(), "wcag2a".to_string()]).is_empty());
    }

    fn file() -> ComponentsFile {
        serde_json::from_value(json!({
            "formatVersion": 1, "fingerprintVersion": 2, "runId": "r_1", "url": "https://x.test",
            "finalUrl": "https://x.test/", "title": "Shop", "browser": { "name": "chromium", "version": "1" },
            "viewport": { "width": 1280, "height": 800 }, "capturedAt": "2026-01-01T00:00:00Z", "warnings": [],
            "components": [{
                "id": "c_btn", "role": "button", "name": "Buy", "nameIsTemplate": false, "confidence": "high",
                "signals": [], "path": "main", "instances": [{
                    "key": "c_btn:0", "name": "Buy", "tag": "button", "attrs": {}, "selectors": [],
                    "box": { "x": 10, "y": 10, "w": 100, "h": 40 },
                    "state": { "visible": true, "enabled": true, "inViewport": true },
                    "frameId": "main", "inShadowDom": false
                }]
            }]
        }))
        .unwrap()
    }

    fn results(nodes: Vec<Value>) -> AxeResults {
        serde_json::from_value(json!({
            "axeVersion": "4.14.0", "tags": ["wcag2aa"], "incomplete": 3, "errors": [],
            "violations": [{
                "rule": "color-contrast", "impact": "serious", "tags": ["wcag2aa", "wcag143"],
                "help": "Elements must meet minimum colour contrast ratio thresholds",
                "helpUrl": "https://dequeuniversity.com/rules/axe/4.14/color-contrast", "nodes": nodes
            }]
        }))
        .unwrap()
    }

    fn node(component: Option<&str>) -> Value {
        let mut node = json!({ "frameId": "main", "target": "#buy", "box": { "x": 10, "y": 10, "w": 100, "h": 40 } });
        if let Some(id) = component {
            node["componentId"] = json!(id);
        }
        node
    }

    #[test]
    fn a_violation_becomes_a_finding_on_its_component_with_rule_and_wcag() {
        let found = findings(&results(vec![node(Some("c_btn"))]), &file());
        assert_eq!(found.len(), 1);
        let f = &found[0];
        assert_eq!(f.id, "axe.color-contrast:c_btn");
        assert_eq!(f.rule, "axe.color-contrast");
        assert_eq!(f.severity, FindingSeverity::Error);
        assert_eq!(f.component_id, "c_btn");
        assert_eq!(f.instances, ["c_btn:0"]);
        assert_eq!(f.wcag, ["1.4.3"]);
        assert_eq!(f.role, "button");
        assert!(f.help_url.as_deref().unwrap().contains("color-contrast"));
        assert_eq!(f.evidence["source"], "axe-core");
        assert_eq!(f.evidence["axeVersion"], "4.14.0");
        assert!(f.message.ends_with(NOT_A_CLAIM));
    }

    #[test]
    fn elements_in_no_component_make_one_page_level_finding() {
        let found = findings(&results(vec![node(None), node(None)]), &file());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "axe.color-contrast:page");
        assert_eq!(found[0].component_id, PAGE);
        assert_eq!(found[0].evidence["elements"], 2);
        assert!(found[0].instances.is_empty());
    }

    #[test]
    fn the_coverage_note_never_claims_conformance() {
        assert!(COVERAGE.contains("57 percent"));
        assert!(COVERAGE.contains("not a conformance claim"));
    }
}
