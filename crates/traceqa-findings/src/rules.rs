//! The rules. Each takes a `components.json` and returns findings; none looks at the page.
//!
//! False positives cost more trust than misses gain, so each rule states a confidence, skips
//! what is not visible, and every message says it is an automated check.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use traceqa_common::types::{
    Component, ComponentsFile, Confidence, Finding, FindingConfidence, FindingSeverity, Instance,
};

use crate::{clean, RunInput};

/// Rule ids in report order.
pub const RULES: [&str; 8] = [
    "unnamed-control",
    "weak-name-source",
    "fake-button",
    "small-target",
    "covered-at-load",
    "ambiguous-names",
    "missing-autocomplete",
    "browsers-disagree",
];

const NOT_A_CLAIM: &str = "This is an automated check, not a conformance claim.";
const MAX_INSTANCES: usize = 20;
/// WCAG 2.5.8: a target is undersized below this many CSS pixels on either side.
const MIN_TARGET: f64 = 24.0;

/// Roles of things a person operates, as the collector names them.
const INTERACTIVE_ROLES: [&str; 17] = [
    "button",
    "link",
    "checkbox",
    "radio",
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "treeitem",
];

fn is_interactive_role(role: &str) -> bool {
    INTERACTIVE_ROLES.contains(&role)
}

fn wcag_url(slug: &str) -> String {
    format!("https://www.w3.org/WAI/WCAG22/Understanding/{slug}.html")
}

struct Draft<'a> {
    rule: &'static str,
    severity: FindingSeverity,
    confidence: FindingConfidence,
    component: &'a Component,
    instances: Vec<&'a Instance>,
    message: String,
    evidence: Value,
    wcag: &'static [&'static str],
    help: Option<String>,
}

impl Draft<'_> {
    fn build(self) -> Finding {
        let Value::Object(evidence) = self.evidence else {
            unreachable!("evidence is always built as an object")
        };
        Finding {
            id: format!("{}:{}", self.rule, self.component.id),
            rule: self.rule.to_string(),
            severity: self.severity,
            confidence: self.confidence,
            component_id: self.component.id.clone(),
            instances: self
                .instances
                .iter()
                .take(MAX_INSTANCES)
                .map(|i| i.key.clone())
                .collect(),
            role: clean(&self.component.role),
            name: clean(&self.component.name),
            path: clean(&self.component.path),
            message: format!("{} {NOT_A_CLAIM}", self.message),
            evidence: sanitise_map(evidence),
            wcag: self.wcag.iter().map(|w| (*w).to_string()).collect(),
            help_url: self.help,
            suppressed: None,
        }
    }
}

pub(crate) fn sanitise_map(map: Map<String, Value>) -> Map<String, Value> {
    map.into_iter().map(|(k, v)| (k, sanitise(v))).collect()
}

/// Page text reaches evidence: control and bidi characters are removed from every string.
fn sanitise(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(clean(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(sanitise).collect()),
        Value::Object(map) => Value::Object(sanitise_map(map)),
        other => other,
    }
}

fn shown(instance: &Instance) -> impl Iterator<Item = &Instance> {
    std::iter::once(instance).filter(|i| i.state.visible)
}

fn visible(component: &Component) -> impl Iterator<Item = &Instance> {
    component.instances.iter().flat_map(shown)
}

/// Every finding of every per-target rule, unsorted.
pub fn target_findings(file: &ComponentsFile) -> Vec<Finding> {
    let mut out = Vec::new();
    unnamed_control(file, &mut out);
    weak_name_source(file, &mut out);
    fake_button(file, &mut out);
    small_target(file, &mut out);
    covered_at_load(file, &mut out);
    ambiguous_names(file, &mut out);
    missing_autocomplete(file, &mut out);
    out
}

/// Findings that need every target of a run: `browsers-disagree`.
pub fn run_findings(run: &RunInput) -> Vec<Finding> {
    let mut out = Vec::new();
    crate::cross_browser::browsers_disagree(run, &mut out);
    out
}

fn unnamed_control(file: &ComponentsFile, out: &mut Vec<Finding>) {
    for component in &file.components {
        if !is_interactive_role(&component.role) {
            continue;
        }
        let bare: Vec<&Instance> = visible(component).filter(|i| i.name.trim().is_empty()).collect();
        if bare.is_empty() {
            continue;
        }
        let (severity, confidence) = match component.confidence {
            Confidence::High => (FindingSeverity::Error, FindingConfidence::High),
            Confidence::Low => (FindingSeverity::Warning, FindingConfidence::Low),
        };
        out.push(
            Draft {
                rule: "unnamed-control",
                severity,
                confidence,
                component,
                message: format!(
                    "A {} has no accessible name, so a screen reader announces it without saying what it does.",
                    clean(&component.role)
                ),
                evidence: json!({ "unnamedInstances": bare.len(), "tag": bare[0].tag, "signals": component.signals }),
                instances: bare,
                wcag: &["4.1.2"],
                help: Some(wcag_url("name-role-value")),
            }
            .build(),
        );
    }
}

fn weak_name_source(file: &ComponentsFile, out: &mut Vec<Finding>) {
    for component in &file.components {
        if !is_interactive_role(&component.role) {
            continue;
        }
        let weak: Vec<&Instance> = visible(component)
            .filter(|i| !i.name.trim().is_empty() && matches!(i.name_source.as_deref(), Some("placeholder" | "title")))
            .collect();
        if weak.is_empty() {
            continue;
        }
        let source = weak[0].name_source.clone().unwrap_or_default();
        out.push(
            Draft {
                rule: "weak-name-source",
                severity: FindingSeverity::Warning,
                confidence: FindingConfidence::Medium,
                component,
                message: format!(
                    "The only source of the name is the {source} attribute. A {source} is not a visible label: it can vanish when typing or is not shown on touch screens."
                ),
                evidence: json!({ "nameSource": source, "weakInstances": weak.len() }),
                instances: weak,
                wcag: &["4.1.2", "3.3.2"],
                help: Some(wcag_url("labels-or-instructions")),
            }
            .build(),
        );
    }
}

fn fake_button(file: &ComponentsFile, out: &mut Vec<Finding>) {
    for component in &file.components {
        let looks_clickable = component
            .signals
            .iter()
            .any(|s| s == "cursor-pointer" || s == "onclick-attr");
        if component.confidence != Confidence::Low || !looks_clickable {
            continue;
        }
        let shown: Vec<&Instance> = visible(component).collect();
        if shown.is_empty() {
            continue;
        }
        let reachable = component.signals.iter().any(|s| s == "tabindex");
        let severity = if reachable {
            FindingSeverity::Warning
        } else {
            FindingSeverity::Error
        };
        let tabindex = shown[0].attrs.get("tabindex").cloned();
        let mut evidence = json!({ "signals": component.signals, "keyboardReachable": reachable });
        if let Some(value) = tabindex {
            evidence["tabindex"] = Value::String(value);
        }
        let message = if reachable {
            "This element acts like a button but has no button role. It can be focused, but a screen reader does not announce it as a button."
        } else {
            "This element acts like a button (a pointer cursor or a click handler) but is not a button or link and cannot be reached with the keyboard."
        };
        out.push(
            Draft {
                rule: "fake-button",
                severity,
                confidence: FindingConfidence::Low,
                component,
                message: message.to_string(),
                evidence,
                instances: shown,
                wcag: &["4.1.2", "2.1.1"],
                help: Some(wcag_url("keyboard")),
            }
            .build(),
        );
    }
}

struct Target<'a> {
    instance: &'a Instance,
    centre: (f64, f64),
}

fn small_target(file: &ComponentsFile, out: &mut Vec<Finding>) {
    let all: Vec<Target<'_>> = file
        .components
        .iter()
        .flat_map(visible)
        .map(|instance| Target {
            instance,
            centre: (
                instance.box_.x + instance.box_.w / 2.0,
                instance.box_.y + instance.box_.h / 2.0,
            ),
        })
        .collect();
    let undersized = |t: &Target<'_>| t.instance.box_.w < MIN_TARGET || t.instance.box_.h < MIN_TARGET;

    for component in &file.components {
        let mut failing: Vec<&Instance> = Vec::new();
        for instance in visible(component).filter(|i| i.state.enabled) {
            let me = Target {
                instance,
                centre: (
                    instance.box_.x + instance.box_.w / 2.0,
                    instance.box_.y + instance.box_.h / 2.0,
                ),
            };
            if !undersized(&me) {
                continue;
            }
            let crowded = all.iter().filter(|o| o.instance.key != instance.key).any(|other| {
                if undersized(other) && distance(me.centre, other.centre) < MIN_TARGET {
                    return true;
                }
                // A box that contains this one is its container, not a neighbour.
                !contains(other.instance, instance) && circle_touches_box(me.centre, MIN_TARGET / 2.0, other.instance)
            });
            if crowded {
                failing.push(instance);
            }
        }
        if failing.is_empty() {
            continue;
        }
        let smallest = failing
            .iter()
            .min_by(|a, b| (a.box_.w * a.box_.h).total_cmp(&(b.box_.w * b.box_.h)))
            .copied()
            .unwrap_or(failing[0]);
        let confidence = if component.role == "link"
            || (smallest.tag == "input" && matches!(component.role.as_str(), "checkbox" | "radio"))
        {
            // Inline links and browser-drawn checkboxes are exceptions the page cannot show.
            FindingConfidence::Low
        } else {
            FindingConfidence::Medium
        };
        out.push(
            Draft {
                rule: "small-target",
                severity: FindingSeverity::Warning,
                confidence,
                component,
                message: format!(
                    "The target is {}x{} CSS pixels, below the 24x24 minimum, and sits close to other targets.",
                    smallest.box_.w, smallest.box_.h
                ),
                evidence: json!({
                    "size": { "w": smallest.box_.w, "h": smallest.box_.h },
                    "minSize": 24,
                    "spacingOk": false,
                    "undersizedInstances": failing.len(),
                }),
                instances: failing,
                wcag: &["2.5.8"],
                help: Some(wcag_url("target-size-minimum")),
            }
            .build(),
        );
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// True when `outer`'s box holds all of `inner`'s.
fn contains(outer: &Instance, inner: &Instance) -> bool {
    let (o, i) = (&outer.box_, &inner.box_);
    o.x <= i.x && o.y <= i.y && o.x + o.w >= i.x + i.w && o.y + o.h >= i.y + i.h
}

/// True when the circle overlaps the box (touching does not count).
fn circle_touches_box(centre: (f64, f64), radius: f64, other: &Instance) -> bool {
    let b = &other.box_;
    let nearest = (centre.0.clamp(b.x, b.x + b.w), centre.1.clamp(b.y, b.y + b.h));
    distance(centre, nearest) < radius
}

fn covered_at_load(file: &ComponentsFile, out: &mut Vec<Finding>) {
    for component in &file.components {
        let covered: Vec<&Instance> = visible(component)
            .filter(|i| i.state.in_viewport && i.state.covered == Some(true))
            .collect();
        if covered.is_empty() {
            continue;
        }
        let by = covered[0].state.covered_by.clone().unwrap_or_default();
        out.push(
            Draft {
                rule: "covered-at-load",
                severity: FindingSeverity::Warning,
                confidence: FindingConfidence::Medium,
                component,
                message: "Another element lies on top of this one when the page has loaded, so a click would not reach it. It may be an open dialog or banner that is meant to be there.".to_string(),
                evidence: json!({ "coveredBy": by, "coveredInstances": covered.len() }),
                instances: covered,
                wcag: &[],
                help: None,
            }
            .build(),
        );
    }
}

fn normalised(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Links with the same words that lead to different places. The destination is the `href`, or
/// the component when there is none. Components with a templated name are skipped: their names
/// differ by design.
fn ambiguous_names(file: &ComponentsFile, out: &mut Vec<Finding>) {
    let mut groups: BTreeMap<String, Vec<&Component>> = BTreeMap::new();
    for component in &file.components {
        if component.role != "link" || component.name_is_template || component.name_changes == Some(true) {
            continue;
        }
        let name = normalised(&component.name);
        if !name.is_empty() {
            groups.entry(name).or_default().push(component);
        }
    }
    for (name, mut members) in groups {
        members.sort_by(|a, b| a.id.cmp(&b.id));
        let mut destinations: BTreeSet<String> = BTreeSet::new();
        let mut keys: Vec<&Instance> = Vec::new();
        for component in &members {
            for instance in visible(component) {
                let destination = instance
                    .attrs
                    .get("href")
                    .map(|h| h.trim())
                    .filter(|h| !h.is_empty())
                    .map_or_else(|| format!("component:{}", component.id), str::to_string);
                destinations.insert(destination);
                keys.push(instance);
            }
        }
        if destinations.len() < 2 {
            continue;
        }
        let first = members[0];
        out.push(
            Draft {
                rule: "ambiguous-names",
                severity: FindingSeverity::Info,
                confidence: FindingConfidence::Medium,
                component: first,
                message: format!(
                    "{} links are named \"{}\" but lead to different places, so out of context they cannot be told apart.",
                    destinations.len(),
                    clean(&first.name)
                ),
                evidence: json!({
                    "name": name,
                    "destinationCount": destinations.len(),
                    "destinations": destinations.iter().take(10).collect::<Vec<_>>(),
                    "components": members.iter().take(10).map(|c| c.id.as_str()).collect::<Vec<_>>(),
                }),
                instances: keys,
                wcag: &["2.4.4"],
                help: Some(wcag_url("link-purpose-in-context")),
            }
            .build(),
        );
    }
}

fn missing_autocomplete(file: &ComponentsFile, out: &mut Vec<Finding>) {
    for component in &file.components {
        let bare: Vec<&Instance> = visible(component)
            .filter(|i| {
                i.tag == "input"
                    && i.attrs
                        .get("type")
                        .is_some_and(|t| matches!(t.trim().to_ascii_lowercase().as_str(), "password" | "email" | "tel"))
                    && i.attrs.get("autocomplete").is_none_or(|a| a.trim().is_empty())
            })
            .collect();
        if bare.is_empty() {
            continue;
        }
        let kind = bare[0]
            .attrs
            .get("type")
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_default();
        out.push(
            Draft {
                rule: "missing-autocomplete",
                severity: FindingSeverity::Info,
                confidence: FindingConfidence::Low,
                component,
                message: format!(
                    "This {kind} field has no autocomplete attribute, so browsers and assistive tools cannot fill it in for the person."
                ),
                evidence: json!({ "inputType": kind, "instances": bare.len() }),
                instances: bare,
                wcag: &["1.3.5"],
                help: Some(wcag_url("identify-input-purpose")),
            }
            .build(),
        );
    }
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // test builders take JSON by value to read like the data they make
mod tests {
    use super::*;
    use crate::analyse_target;
    use crate::Options;

    /// One instance as JSON: tag, name, attributes, box and the state flags.
    fn inst(key: &str, name: &str, attrs: Value, box_: [f64; 4]) -> Value {
        json!({
            "key": key, "name": name, "tag": "button", "attrs": attrs, "selectors": [],
            "box": { "x": box_[0], "y": box_[1], "w": box_[2], "h": box_[3] },
            "state": { "visible": true, "enabled": true, "inViewport": true },
            "frameId": "main", "inShadowDom": false
        })
    }

    fn comp(id: &str, role: &str, name: &str, confidence: &str, signals: &[&str], instances: Vec<Value>) -> Value {
        json!({
            "id": id, "role": role, "name": name, "nameIsTemplate": false, "confidence": confidence,
            "signals": signals, "path": "main", "instances": instances
        })
    }

    fn file(components: Vec<Value>) -> ComponentsFile {
        serde_json::from_value(json!({
            "formatVersion": 1, "fingerprintVersion": 2, "runId": "r_1", "url": "https://x.test",
            "finalUrl": "https://x.test/", "title": "t", "browser": { "name": "chromium", "version": "1" },
            "viewport": { "width": 1280, "height": 800 }, "capturedAt": "2026-01-01T00:00:00Z",
            "components": components, "warnings": []
        }))
        .unwrap()
    }

    fn ids(file: &ComponentsFile, rule: &str) -> Vec<String> {
        analyse_target(file, &Options::default())
            .findings
            .into_iter()
            .filter(|f| f.rule == rule)
            .map(|f| f.id)
            .collect()
    }

    fn one(file: &ComponentsFile, rule: &str) -> Finding {
        let mut found: Vec<Finding> = analyse_target(file, &Options::default())
            .findings
            .into_iter()
            .filter(|f| f.rule == rule)
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        found.remove(0)
    }

    // ---- unnamed-control

    #[test]
    fn unnamed_native_button_is_an_error_and_named_is_fine() {
        let f = file(vec![
            comp(
                "c_000000000001",
                "button",
                "",
                "high",
                &["native"],
                vec![inst("c_000000000001:0", "", json!({}), [0.0, 0.0, 40.0, 40.0])],
            ),
            comp(
                "c_000000000002",
                "button",
                "Go",
                "high",
                &["native"],
                vec![inst("c_000000000002:0", "Go", json!({}), [100.0, 0.0, 40.0, 40.0])],
            ),
        ]);
        let finding = one(&f, "unnamed-control");
        assert_eq!(finding.id, "unnamed-control:c_000000000001");
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert!(finding.message.contains("automated check"));
    }

    #[test]
    fn unnamed_low_confidence_is_a_warning_and_hidden_is_skipped() {
        let mut hidden = inst("c_000000000003:0", "", json!({}), [0.0, 0.0, 40.0, 40.0]);
        hidden["state"]["visible"] = json!(false);
        let f = file(vec![
            comp(
                "c_000000000001",
                "button",
                "",
                "low",
                &["cursor-pointer"],
                vec![inst("c_000000000001:0", "", json!({}), [0.0, 0.0, 40.0, 40.0])],
            ),
            comp("c_000000000003", "button", "", "high", &["native"], vec![hidden]),
        ]);
        let finding = one(&f, "unnamed-control");
        assert_eq!(finding.severity, FindingSeverity::Warning);
        assert_eq!(finding.confidence, FindingConfidence::Low);
        assert_eq!(ids(&f, "unnamed-control").len(), 1);
    }

    #[test]
    fn unnamed_generic_elements_are_not_controls() {
        let f = file(vec![comp(
            "c_000000000001",
            "generic",
            "",
            "low",
            &["cursor-pointer"],
            vec![inst("c_000000000001:0", "", json!({}), [0.0, 0.0, 40.0, 40.0])],
        )]);
        assert!(ids(&f, "unnamed-control").is_empty());
    }

    // ---- weak-name-source

    fn named_by(source: Option<&str>) -> ComponentsFile {
        let mut i = inst("c_000000000001:0", "Email", json!({}), [0.0, 0.0, 200.0, 40.0]);
        if let Some(source) = source {
            i["nameSource"] = json!(source);
        }
        file(vec![comp(
            "c_000000000001",
            "textbox",
            "Email",
            "high",
            &["native"],
            vec![i],
        )])
    }

    #[test]
    fn placeholder_and_title_only_names_are_weak() {
        assert_eq!(
            ids(&named_by(Some("placeholder")), "weak-name-source"),
            ["weak-name-source:c_000000000001"]
        );
        assert_eq!(ids(&named_by(Some("title")), "weak-name-source").len(), 1);
    }

    #[test]
    fn a_label_with_the_same_text_as_the_placeholder_is_not_weak() {
        assert!(ids(&named_by(Some("label")), "weak-name-source").is_empty());
        assert!(ids(&named_by(Some("aria-label")), "weak-name-source").is_empty());
        // Files from before nameSource existed say nothing about it.
        assert!(ids(&named_by(None), "weak-name-source").is_empty());
    }

    // ---- fake-button

    fn fake(signals: &[&str], attrs: Value) -> ComponentsFile {
        file(vec![comp(
            "c_000000000001",
            "generic",
            "Buy",
            "low",
            signals,
            vec![inst("c_000000000001:0", "Buy", attrs, [0.0, 0.0, 80.0, 40.0])],
        )])
    }

    #[test]
    fn fake_buttons_by_signal() {
        let pointer = one(&fake(&["cursor-pointer"], json!({})), "fake-button");
        assert_eq!(pointer.severity, FindingSeverity::Error);
        assert_eq!(pointer.confidence, FindingConfidence::Low);
        assert_eq!(
            one(&fake(&["onclick-attr"], json!({})), "fake-button").severity,
            FindingSeverity::Error
        );
        let focusable = one(
            &fake(&["tabindex", "cursor-pointer"], json!({ "tabindex": "0" })),
            "fake-button",
        );
        assert_eq!(focusable.severity, FindingSeverity::Warning);
        assert_eq!(focusable.evidence["tabindex"], "0");
        assert_eq!(focusable.evidence["keyboardReachable"], true);
    }

    #[test]
    fn tabindex_minus_one_is_not_keyboard_reachable() {
        // The collector gives no tabindex signal for -1, so this is an error, with the value shown.
        let finding = one(&fake(&["onclick-attr"], json!({ "tabindex": "-1" })), "fake-button");
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert_eq!(finding.evidence["tabindex"], "-1");
    }

    #[test]
    fn a_real_button_or_a_tabindex_only_element_is_not_a_fake_button() {
        let real = file(vec![comp(
            "c_000000000001",
            "button",
            "Buy",
            "high",
            &["native", "onclick-attr"],
            vec![inst("c_000000000001:0", "Buy", json!({}), [0.0, 0.0, 80.0, 40.0])],
        )]);
        assert!(ids(&real, "fake-button").is_empty());
        assert!(ids(&fake(&["tabindex"], json!({ "tabindex": "0" })), "fake-button").is_empty());
    }

    // ---- small-target

    fn lone(w: f64, h: f64) -> ComponentsFile {
        file(vec![comp(
            "c_000000000001",
            "button",
            "x",
            "high",
            &["native"],
            vec![inst("c_000000000001:0", "x", json!({}), [100.0, 100.0, w, h])],
        )])
    }

    /// A target with neighbours 0.25 px to its right and below it. A 24 px circle reaches 12 px from
    /// the centre, so only a side shorter than 24 px lets a neighbour that close fall inside it.
    fn crowded(w: f64, h: f64) -> ComponentsFile {
        file(vec![
            comp(
                "c_000000000001",
                "button",
                "x",
                "high",
                &["native"],
                vec![inst("c_000000000001:0", "x", json!({}), [100.0, 100.0, w, h])],
            ),
            comp(
                "c_000000000002",
                "button",
                "y",
                "high",
                &["native"],
                vec![inst(
                    "c_000000000002:0",
                    "y",
                    json!({}),
                    [100.0 + w + 0.25, 100.0, 60.0, 40.0],
                )],
            ),
            comp(
                "c_000000000003",
                "button",
                "z",
                "high",
                &["native"],
                vec![inst(
                    "c_000000000003:0",
                    "z",
                    json!({}),
                    [100.0, 100.0 + h + 0.25, 60.0, 40.0],
                )],
            ),
        ])
    }

    #[test]
    fn box_sizes_at_the_24_pixel_edge() {
        for (w, h, flagged) in [
            (23.0, 23.0, true),
            (24.0, 24.0, false),
            (24.0, 23.0, true),
            (23.0, 24.0, true),
        ] {
            let found = ids(&crowded(w, h), "small-target");
            assert_eq!(!found.is_empty(), flagged, "{w}x{h}: {found:?}");
        }
    }

    #[test]
    fn an_undersized_target_far_from_the_others_passes_the_spacing_exception() {
        assert!(ids(&lone(16.0, 16.0), "small-target").is_empty());
        let far = file(vec![
            comp(
                "c_000000000001",
                "button",
                "x",
                "high",
                &["native"],
                vec![inst("c_000000000001:0", "x", json!({}), [100.0, 100.0, 16.0, 16.0])],
            ),
            comp(
                "c_000000000002",
                "button",
                "y",
                "high",
                &["native"],
                vec![inst("c_000000000002:0", "y", json!({}), [300.0, 100.0, 16.0, 16.0])],
            ),
        ]);
        assert!(ids(&far, "small-target").is_empty());
    }

    #[test]
    fn two_undersized_targets_with_touching_circles_fail_and_exactly_touching_pass() {
        let near = |gap: f64| {
            file(vec![
                comp(
                    "c_000000000001",
                    "button",
                    "x",
                    "high",
                    &["native"],
                    vec![inst("c_000000000001:0", "x", json!({}), [100.0, 100.0, 16.0, 16.0])],
                ),
                comp(
                    "c_000000000002",
                    "button",
                    "y",
                    "high",
                    &["native"],
                    vec![inst(
                        "c_000000000002:0",
                        "y",
                        json!({}),
                        [100.0 + 16.0 + gap, 100.0, 16.0, 16.0],
                    )],
                ),
            ])
        };
        // Centres are 16 + gap apart; the circles intersect below 24.
        assert_eq!(ids(&near(4.0), "small-target").len(), 2);
        assert!(ids(&near(8.0), "small-target").is_empty());
    }

    #[test]
    fn a_target_inside_a_larger_one_is_not_crowded_by_its_container() {
        let f = file(vec![
            comp(
                "c_000000000001",
                "button",
                "card",
                "high",
                &["native"],
                vec![inst("c_000000000001:0", "card", json!({}), [0.0, 0.0, 300.0, 200.0])],
            ),
            comp(
                "c_000000000002",
                "link",
                "x",
                "high",
                &["native"],
                vec![inst("c_000000000002:0", "x", json!({}), [100.0, 100.0, 16.0, 16.0])],
            ),
        ]);
        assert!(ids(&f, "small-target").is_empty());
    }

    #[test]
    fn small_target_confidence_and_evidence() {
        let finding = one(&crowded(16.0, 16.0), "small-target");
        assert_eq!(finding.confidence, FindingConfidence::Medium);
        assert_eq!(finding.evidence["size"]["w"], 16.0);
        assert_eq!(finding.evidence["spacingOk"], false);
        assert_eq!(finding.wcag, ["2.5.8"]);
        let mut link = crowded(16.0, 16.0);
        link.components[0].role = "link".into();
        assert_eq!(one(&link, "small-target").confidence, FindingConfidence::Low);
        let mut check = crowded(13.0, 13.0);
        check.components[0].role = "checkbox".into();
        check.components[0].instances[0].tag = "input".into();
        assert_eq!(one(&check, "small-target").confidence, FindingConfidence::Low);
    }

    #[test]
    fn hidden_and_disabled_undersized_targets_are_skipped() {
        let mut f = crowded(16.0, 16.0);
        f.components[0].instances[0].state.visible = false;
        assert!(ids(&f, "small-target").is_empty());
        let mut g = crowded(16.0, 16.0);
        g.components[0].instances[0].state.enabled = false;
        assert!(ids(&g, "small-target").is_empty());
    }

    // ---- covered-at-load

    #[test]
    fn covered_visible_in_viewport_instances_are_reported() {
        let mut covered = inst("c_000000000001:0", "Buy", json!({}), [0.0, 0.0, 80.0, 40.0]);
        covered["state"]["covered"] = json!(true);
        covered["state"]["coveredBy"] = json!("#banner");
        let mut below = inst("c_000000000002:0", "Below", json!({}), [0.0, 5000.0, 80.0, 40.0]);
        below["state"]["covered"] = json!(true);
        below["state"]["inViewport"] = json!(false);
        let f = file(vec![
            comp("c_000000000001", "button", "Buy", "high", &["native"], vec![covered]),
            comp("c_000000000002", "button", "Below", "high", &["native"], vec![below]),
            comp(
                "c_000000000003",
                "button",
                "Free",
                "high",
                &["native"],
                vec![inst("c_000000000003:0", "Free", json!({}), [0.0, 100.0, 80.0, 40.0])],
            ),
        ]);
        let finding = one(&f, "covered-at-load");
        assert_eq!(finding.id, "covered-at-load:c_000000000001");
        assert_eq!(finding.evidence["coveredBy"], "#banner");
        assert_eq!(finding.confidence, FindingConfidence::Medium);
    }

    // ---- ambiguous-names

    fn link(id: &str, name: &str, href: &str, y: f64) -> Value {
        let mut i = inst(&format!("{id}:0"), name, json!({ "href": href }), [0.0, y, 100.0, 30.0]);
        i["tag"] = json!("a");
        comp(id, "link", name, "high", &["native"], vec![i])
    }

    #[test]
    fn two_read_more_links_with_different_hrefs_are_ambiguous() {
        let f = file(vec![
            link("c_000000000002", "Read more", "/a", 0.0),
            link("c_000000000001", "Read  MORE", "/b", 100.0),
        ]);
        let finding = one(&f, "ambiguous-names");
        // Anchored on the smallest component id so the id is stable.
        assert_eq!(finding.id, "ambiguous-names:c_000000000001");
        assert_eq!(finding.severity, FindingSeverity::Info);
        assert_eq!(finding.evidence["destinationCount"], 2);
        assert_eq!(finding.instances.len(), 2);
    }

    #[test]
    fn the_same_name_and_the_same_destination_is_fine() {
        let f = file(vec![
            link("c_000000000001", "Contact", "/contact", 0.0),
            link("c_000000000002", "Contact", "/contact", 900.0),
        ]);
        assert!(ids(&f, "ambiguous-names").is_empty());
    }

    #[test]
    fn one_component_with_many_hrefs_is_ambiguous_and_templated_names_are_not() {
        let mut many = link("c_000000000001", "Read more", "/a", 0.0);
        let mut second = inst(
            "c_000000000001:1",
            "Read more",
            json!({ "href": "/b" }),
            [0.0, 100.0, 100.0, 30.0],
        );
        second["tag"] = json!("a");
        many["instances"].as_array_mut().unwrap().push(second);
        assert_eq!(ids(&file(vec![many.clone()]), "ambiguous-names").len(), 1);
        many["nameIsTemplate"] = json!(true);
        many["name"] = json!("*");
        let other = link("c_000000000002", "*", "/c", 300.0);
        assert!(ids(&file(vec![many, other]), "ambiguous-names").is_empty());
    }

    #[test]
    fn buttons_with_the_same_name_are_not_reported() {
        let f = file(vec![
            comp(
                "c_000000000001",
                "button",
                "Close",
                "high",
                &["native"],
                vec![inst("c_000000000001:0", "Close", json!({}), [0.0, 0.0, 40.0, 40.0])],
            ),
            comp(
                "c_000000000002",
                "button",
                "Close",
                "high",
                &["native"],
                vec![inst("c_000000000002:0", "Close", json!({}), [0.0, 500.0, 40.0, 40.0])],
            ),
        ]);
        assert!(ids(&f, "ambiguous-names").is_empty());
    }

    // ---- missing-autocomplete

    fn field(kind: &str, extra: Value) -> ComponentsFile {
        let mut attrs = json!({ "type": kind });
        if let Value::Object(more) = extra {
            attrs.as_object_mut().unwrap().extend(more);
        }
        let mut i = inst("c_000000000001:0", "Field", attrs, [0.0, 0.0, 200.0, 30.0]);
        i["tag"] = json!("input");
        file(vec![comp(
            "c_000000000001",
            "textbox",
            "Field",
            "high",
            &["native"],
            vec![i],
        )])
    }

    #[test]
    fn password_email_and_tel_without_autocomplete_are_reported() {
        for kind in ["password", "email", "tel", "EMAIL"] {
            assert_eq!(ids(&field(kind, json!({})), "missing-autocomplete").len(), 1, "{kind}");
        }
        assert!(ids(&field("text", json!({})), "missing-autocomplete").is_empty());
        assert!(ids(
            &field("password", json!({ "autocomplete": "current-password" })),
            "missing-autocomplete"
        )
        .is_empty());
        assert_eq!(
            ids(&field("email", json!({ "autocomplete": " " })), "missing-autocomplete").len(),
            1
        );
        let finding = one(&field("password", json!({})), "missing-autocomplete");
        assert_eq!(
            (finding.severity, finding.confidence),
            (FindingSeverity::Info, FindingConfidence::Low)
        );
        assert_eq!(finding.wcag, ["1.3.5"]);
    }

    // ---- shared behaviour

    #[test]
    fn page_text_is_sanitised_in_name_message_and_evidence() {
        let name = "Go\u{202e}evil\u{0007}\nnow";
        let mut f = file(vec![comp(
            "c_000000000001",
            "button",
            name,
            "high",
            &["native"],
            vec![inst("c_000000000001:0", "", json!({}), [0.0, 0.0, 40.0, 40.0])],
        )]);
        f.components[0].instances[0].state.covered = Some(true);
        f.components[0].instances[0].state.covered_by = Some("#a\u{202e}b".into());
        let covered = one(&f, "covered-at-load");
        assert!(
            !covered.name.contains('\u{202e}') && !covered.name.contains('\u{7}'),
            "{}",
            covered.name
        );
        assert_eq!(covered.evidence["coveredBy"], "#ab");
    }

    #[test]
    fn a_finding_lists_at_most_20_instances() {
        let instances: Vec<Value> = (0..30)
            .map(|n| {
                inst(
                    &format!("c_000000000001:{n}"),
                    "",
                    json!({}),
                    [0.0, 50.0 * f64::from(n), 40.0, 40.0],
                )
            })
            .collect();
        let f = file(vec![comp(
            "c_000000000001",
            "button",
            "",
            "high",
            &["native"],
            instances,
        )]);
        let finding = one(&f, "unnamed-control");
        assert_eq!(finding.instances.len(), 20);
        assert_eq!(finding.evidence["unnamedInstances"], 30);
    }
}
