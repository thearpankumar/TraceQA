//! The run-level rule `browsers-disagree`: the same component id behaves differently across
//! browsers at one viewport.
//!
//! Targets are grouped by viewport (two sizes are independent, and a component can disagree at
//! one and not the other). Within a group of at least two targets, every component id seen in
//! any of them is compared with the diff core ([`traceqa_diff::summarise`] and
//! [`traceqa_diff::changed_fields`]). One finding per component and viewport:
//!
//! - present in some browsers only: `warning`, confidence `high`;
//! - present in all, but none, some or all instances visible (or enabled) differs between
//!   browsers: `warning`, confidence `medium`;
//! - present in all, only the number of instances differs: `info`, confidence `medium`.
//!
//! No browser is taken as the reference: the evidence lists every browser's values. When a
//! component is missing in one browser, a removed/added pair found by [`traceqa_diff::pair`]
//! (same path, another role or name) is shown as `similar`, because a component whose role
//! differs between browsers has a different id in each and would otherwise look unrelated.
//!
//! A page that was read before it settled makes browsers disagree for no real reason, which is
//! why the engine's `browsers_disagree` warning advises a settle time.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use traceqa_diff::pair::{pair, Candidate};
use traceqa_diff::{changed_fields, summarise, ComponentSummary};
use traceqa_protocol::types::{Component, Finding, FindingConfidence, FindingSeverity};

use crate::rules::sanitise_map;
use crate::{clean, RunInput, TargetInput};

pub(crate) const RULE: &str = "browsers-disagree";
const NOT_A_CLAIM: &str = "This is an automated check, not a conformance claim.";

struct Side<'a> {
    label: String,
    by_id: BTreeMap<&'a str, &'a Component>,
}

pub(crate) fn browsers_disagree(run: &RunInput, out: &mut Vec<Finding>) {
    let mut groups: Vec<((u64, u64), Vec<&TargetInput>)> = Vec::new();
    for target in &run.targets {
        let size = (target.file.viewport.width, target.file.viewport.height);
        match groups.iter_mut().find(|(seen, _)| *seen == size) {
            Some((_, group)) => group.push(target),
            None => groups.push((size, vec![target])),
        }
    }
    for ((width, height), group) in groups {
        if group.len() >= 2 {
            compare_group(width, height, &group, out);
        }
    }
}

fn compare_group(width: u64, height: u64, group: &[&TargetInput], out: &mut Vec<Finding>) {
    let names: Vec<String> = group.iter().map(|t| clean(&t.file.browser.name)).collect();
    let sides: Vec<Side> = group
        .iter()
        .zip(&names)
        .map(|(target, name)| Side {
            // The same browser asked for twice is told apart by its folder.
            label: if names.iter().filter(|n| *n == name).count() > 1 {
                clean(&target.dir)
            } else {
                name.clone()
            },
            by_id: target.file.components.iter().map(|c| (c.id.as_str(), c)).collect(),
        })
        .collect();
    let summaries: Vec<BTreeMap<&str, ComponentSummary>> = sides
        .iter()
        .map(|s| s.by_id.iter().map(|(id, c)| (*id, summarise(c))).collect())
        .collect();
    let similar = similar_pairs(&sides, &summaries);

    let ids: BTreeSet<&str> = sides.iter().flat_map(|s| s.by_id.keys().copied()).collect();
    for id in ids {
        let present: Vec<usize> = (0..sides.len()).filter(|i| sides[*i].by_id.contains_key(id)).collect();
        let first = present[0];
        let component = sides[first].by_id[id];

        let mut differs: Vec<&str> = Vec::new();
        let mut severity = FindingSeverity::Info;
        let mut confidence = FindingConfidence::Medium;
        if present.len() < sides.len() {
            differs.push("presence");
            severity = FindingSeverity::Warning;
            confidence = FindingConfidence::High;
        }
        if present.len() >= 2 {
            let base = &summaries[first][id];
            let mut fields: BTreeSet<String> = BTreeSet::new();
            for i in &present[1..] {
                fields.extend(changed_fields(base, &summaries[*i][id]));
            }
            // None, some or all instances: a different count of the same kind is not a state change.
            let shape = |count: u64, total: u64| u8::from(count > 0) + u8::from(count == total);
            let differ = |count: fn(&ComponentSummary) -> u64| {
                let first_shape = shape(count(base), base.instances);
                present[1..].iter().any(|i| {
                    let s = &summaries[*i][id];
                    shape(count(s), s.instances) != first_shape
                })
            };
            if fields.contains("visible") && differ(|s| s.visible) {
                differs.push("visible");
                severity = FindingSeverity::Warning;
            }
            if fields.contains("enabled") && differ(|s| s.enabled) {
                differs.push("enabled");
                severity = FindingSeverity::Warning;
            }
            if fields.contains("instances") {
                differs.push("instances");
            }
        }
        if differs.is_empty() {
            continue;
        }

        let mut browsers = Map::new();
        let mut missing_in = Vec::new();
        let mut similar_in = Map::new();
        for (i, side) in sides.iter().enumerate() {
            match summaries[i].get(id) {
                Some(s) => {
                    browsers.insert(
                        side.label.clone(),
                        json!({ "present": true, "instances": s.instances, "visible": s.visible, "enabled": s.enabled }),
                    );
                }
                None => {
                    browsers.insert(side.label.clone(), json!({ "present": false }));
                    missing_in.push(side.label.clone());
                    if let Some((other, reason)) = similar.get(&(i, id.to_string())) {
                        let c = side.by_id[other.as_str()];
                        similar_in.insert(
                            side.label.clone(),
                            json!({ "id": other, "role": c.role, "name": c.name, "reason": reason }),
                        );
                    }
                }
            }
        }
        let present_in: Vec<&str> = present.iter().map(|i| sides[*i].label.as_str()).collect();
        let mut evidence = Map::new();
        evidence.insert("viewport".into(), json!({ "width": width, "height": height }));
        evidence.insert("differs".into(), json!(differs));
        evidence.insert("browsers".into(), Value::Object(browsers));
        if !missing_in.is_empty() {
            evidence.insert("presentIn".into(), json!(present_in));
            evidence.insert("missingIn".into(), json!(missing_in));
        }
        if !similar_in.is_empty() {
            evidence.insert("similar".into(), Value::Object(similar_in));
        }

        let what = if differs[0] == "presence" {
            format!(
                "Found in {} but not in {} at {width}x{height}.",
                present_in.join(", "),
                missing_in.join(", ")
            )
        } else {
            format!(
                "Differs between browsers at {width}x{height} in: {}.",
                differs.join(", ")
            )
        };
        out.push(Finding {
            id: format!("{RULE}:{id}@{width}x{height}"),
            rule: RULE.to_string(),
            severity,
            confidence,
            component_id: id.to_string(),
            instances: Vec::new(),
            role: clean(&component.role),
            name: clean(&component.name),
            path: clean(&component.path),
            message: format!("{what} A page read before it settled can do this; try a settle time. {NOT_A_CLAIM}"),
            evidence: sanitise_map(evidence),
            wcag: Vec::new(),
            help_url: None,
            suppressed: None,
        });
    }
}

/// For a component `X` missing in side `b`, the component of `b` that probably is `X` after a
/// change of role or name, found by pairing what only `a` has with what only `b` has. The first
/// `a` that has an answer wins.
fn similar_pairs<'a>(
    sides: &[Side<'a>],
    summaries: &[BTreeMap<&'a str, ComponentSummary>],
) -> BTreeMap<(usize, String), (String, &'static str)> {
    let mut found = BTreeMap::new();
    for a in 0..sides.len() {
        for b in 0..sides.len() {
            if a == b {
                continue;
            }
            let only = |x: usize, y: usize| -> Vec<Candidate<'_>> {
                summaries[x]
                    .iter()
                    .filter(|(id, _)| !summaries[y].contains_key(*id))
                    .map(|(id, summary)| Candidate { id, summary })
                    .collect()
            };
            let removed = only(a, b);
            let added = only(b, a);
            if removed.is_empty() || added.is_empty() {
                continue;
            }
            for p in pair(&removed, &added) {
                found
                    .entry((b, removed[p.removed].id.to_string()))
                    .or_insert((added[p.added].id.to_string(), p.reason));
            }
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // builders take JSON by value to read like the data they make
mod tests {
    use super::*;
    use serde_json::json;
    use traceqa_protocol::types::ComponentsFile;

    fn component(id: &str, role: &str, name: &str, instances: &[(bool, bool)]) -> Value {
        let list: Vec<Value> = instances
            .iter()
            .enumerate()
            .map(|(n, (visible, enabled))| {
                json!({
                    "key": format!("{id}:{n}"), "name": name, "tag": "button", "attrs": {}, "selectors": [],
                    "box": { "x": 0, "y": 0, "w": 40, "h": 40 },
                    "state": { "visible": visible, "enabled": enabled, "inViewport": true },
                    "frameId": "main", "inShadowDom": false
                })
            })
            .collect();
        json!({
            "id": id, "role": role, "name": name, "nameIsTemplate": false, "confidence": "high",
            "signals": ["native"], "path": format!("main/{name}"), "instances": list
        })
    }

    fn one(id: &str) -> Value {
        component(id, "button", id, &[(true, true)])
    }

    fn target(dir: &str, browser: &str, width: u64, components: Vec<Value>) -> TargetInput {
        let file: ComponentsFile = serde_json::from_value(json!({
            "formatVersion": 1, "fingerprintVersion": 2, "runId": "r_1", "url": "https://x.test",
            "finalUrl": "https://x.test/", "title": "t", "browser": { "name": browser, "version": "1" },
            "viewport": { "width": width, "height": 800 }, "capturedAt": "2026-01-01T00:00:00Z",
            "components": components, "warnings": []
        }))
        .unwrap();
        TargetInput {
            dir: dir.into(),
            file,
            axe: None,
        }
    }

    fn rule(targets: Vec<TargetInput>) -> Vec<Finding> {
        let mut out = Vec::new();
        browsers_disagree(
            &RunInput {
                run_id: "r_1".into(),
                targets,
            },
            &mut out,
        );
        out
    }

    #[test]
    fn an_id_missing_in_one_of_three_browsers_is_a_high_confidence_warning() {
        let out = rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![one("c_a"), one("c_b")]),
            target("firefox-1280x800", "firefox", 1280, vec![one("c_a"), one("c_b")]),
            target("webkit-1280x800", "webkit", 1280, vec![one("c_a")]),
        ]);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.id, "browsers-disagree:c_b@1280x800");
        assert_eq!(f.severity, FindingSeverity::Warning);
        assert_eq!(f.confidence, FindingConfidence::High);
        assert_eq!(f.evidence["missingIn"], json!(["webkit"]));
        assert_eq!(f.evidence["presentIn"], json!(["chromium", "firefox"]));
        assert_eq!(f.evidence["browsers"]["webkit"], json!({ "present": false }));
        assert_eq!(f.evidence["browsers"]["firefox"]["instances"], 1);
        assert!(f.message.contains("automated check"));
    }

    #[test]
    fn a_disabled_control_in_one_browser_is_reported() {
        let off = component("c_a", "button", "Buy", &[(true, false)]);
        let on = component("c_a", "button", "Buy", &[(true, true)]);
        let out = rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![on]),
            target("firefox-1280x800", "firefox", 1280, vec![off]),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].evidence["differs"], json!(["enabled"]));
        assert_eq!(out[0].severity, FindingSeverity::Warning);
        assert_eq!(out[0].evidence["browsers"]["firefox"]["enabled"], 0);
    }

    #[test]
    fn hidden_in_one_browser_is_reported() {
        let hidden = component("c_a", "button", "Buy", &[(false, true)]);
        let shown = component("c_a", "button", "Buy", &[(true, true)]);
        let out = rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![shown]),
            target("firefox-1280x800", "firefox", 1280, vec![hidden]),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].evidence["differs"], json!(["visible"]));
        assert_eq!(out[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn a_different_instance_count_alone_is_info() {
        let two = component("c_a", "button", "c_a", &[(true, true), (true, true)]);
        let out = rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![two]),
            target("firefox-1280x800", "firefox", 1280, vec![one("c_a")]),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, FindingSeverity::Info);
        assert_eq!(out[0].evidence["differs"], json!(["instances"]));
    }

    #[test]
    fn one_browser_or_agreement_yields_nothing() {
        assert!(rule(vec![target("chromium-1280x800", "chromium", 1280, vec![one("c_a")])]).is_empty());
        assert!(rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![one("c_a")]),
            target("firefox-1280x800", "firefox", 1280, vec![one("c_a")]),
        ])
        .is_empty());
        // Two browsers at two different sizes are not compared with each other.
        assert!(rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![one("c_a")]),
            target("firefox-390x800", "firefox", 390, vec![]),
        ])
        .is_empty());
    }

    #[test]
    fn viewports_are_independent_and_ids_name_the_size() {
        let out = rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![one("c_a")]),
            target("chromium-390x800", "chromium", 390, vec![one("c_a")]),
            target("firefox-1280x800", "firefox", 1280, vec![]),
            target("firefox-390x800", "firefox", 390, vec![]),
        ]);
        let ids: Vec<&str> = out.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["browsers-disagree:c_a@1280x800", "browsers-disagree:c_a@390x800"]);
    }

    #[test]
    fn a_role_change_is_shown_as_a_similar_component() {
        let link = component("c_link", "link", "Buy", &[(true, true)]);
        let button = component("c_button", "button", "Buy", &[(true, true)]);
        let out = rule(vec![
            target("chromium-1280x800", "chromium", 1280, vec![button]),
            target("firefox-1280x800", "firefox", 1280, vec![link]),
        ]);
        assert_eq!(out.len(), 2);
        let for_button = out.iter().find(|f| f.component_id == "c_button").unwrap();
        assert_eq!(for_button.evidence["similar"]["firefox"]["id"], "c_link");
        assert_eq!(for_button.evidence["similar"]["firefox"]["role"], "link");
        assert_eq!(for_button.evidence["similar"]["firefox"]["reason"], "role_changed");
    }

    #[test]
    fn the_order_is_deterministic() {
        let build = || {
            rule(vec![
                target("chromium-1280x800", "chromium", 1280, vec![one("c_b"), one("c_a")]),
                target("firefox-1280x800", "firefox", 1280, vec![]),
            ])
        };
        let first = serde_json::to_string(&build()).unwrap();
        assert_eq!(first, serde_json::to_string(&build()).unwrap());
        let ids: Vec<String> = build().into_iter().map(|f| f.component_id).collect();
        assert_eq!(ids, ["c_a", "c_b"]);
    }
}
