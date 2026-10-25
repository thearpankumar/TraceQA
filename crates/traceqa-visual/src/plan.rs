//! Deciding, before any pixel is read, which components can be compared.
//!
//! A component is compared only when both sides have it, the structural diff found it unchanged,
//! it was actually visible and not covered, its name is stable, both sides have a crop, and its box
//! has the same size. Everything else is reported with the reason, so a person can see what was
//! not checked. Pure: no files.

use std::collections::BTreeMap;

use traceqa_diff::{DiffEntryKind, DiffFile};
use traceqa_protocol::types::{Component, ComponentsFile};

/// A box may differ by this many CSS pixels in width or height before the component counts as resized.
pub const SIZE_TOLERANCE: f64 = 1.0;

/// What to do with one component.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Compare the two crops.
    Compare,
    /// The box size differs: report it, compare nothing.
    SizeChanged(String),
    /// Leave it out, for this reason (a `VisualResult.reason` value).
    Skip {
        reason: &'static str,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Planned<'a> {
    pub id: &'a str,
    pub role: &'a str,
    pub name: &'a str,
    /// The crop of each side, relative to its components.json; set when the verdict is `Compare`.
    pub crops: Option<(&'a str, &'a str)>,
    pub verdict: Verdict,
}

fn skip(reason: &'static str) -> Verdict {
    Verdict::Skip { reason, detail: None }
}

/// One verdict per component id on either side, ordered by id. `diff` must be the structural diff
/// of `a` and `b` (A to B).
pub fn plan<'a>(a: &'a ComponentsFile, b: &'a ComponentsFile, diff: &DiffFile) -> Vec<Planned<'a>> {
    let in_a: BTreeMap<&str, &Component> = a.components.iter().map(|c| (c.id.as_str(), c)).collect();
    let in_b: BTreeMap<&str, &Component> = b.components.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut ids: Vec<&str> = in_a.keys().chain(in_b.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .map(|id| match (in_a.get(id), in_b.get(id)) {
            (Some(x), Some(y)) => decide(x, y, diff),
            (Some(only), None) => lone(only, "removed"),
            (None, Some(only)) => lone(only, "added"),
            (None, None) => unreachable!("the id came from one of the two maps"),
        })
        .collect()
}

fn lone<'a>(component: &'a Component, reason: &'static str) -> Planned<'a> {
    Planned {
        id: &component.id,
        role: &component.role,
        name: &component.name,
        crops: None,
        verdict: skip(reason),
    }
}

fn decide<'a>(a: &'a Component, b: &'a Component, diff: &DiffFile) -> Planned<'a> {
    let mut planned = Planned {
        id: &a.id,
        role: &a.role,
        name: &a.name,
        crops: None,
        verdict: Verdict::Compare,
    };
    let either = |test: &dyn Fn(&Component) -> bool| test(a) || test(b);
    let covered = |c: &Component| c.instances.iter().any(|i| i.state.covered == Some(true));
    let no_visible = |c: &Component| !c.instances.iter().any(|i| i.state.visible);
    let changed = diff
        .entries
        .iter()
        .find(|entry| entry.kind == DiffEntryKind::Changed && entry.id == a.id);

    // Masking a rectangle needs the text's box, which the collector does not record, so a component
    // whose name moves is skipped whole.
    planned.verdict = if either(&|c| c.name_changes == Some(true)) {
        skip("name_changes")
    } else if either(&|c| c.name_is_template) {
        skip("name_is_template")
    } else if either(&covered) {
        skip("covered")
    } else if either(&no_visible) {
        skip("not_visible")
    } else if let Some(entry) = changed {
        Verdict::Skip {
            reason: "structurally_changed",
            detail: Some(format!("changed: {}", entry.fields.join(", "))),
        }
    } else if let (Some(crop_a), Some(crop_b)) = (a.crop.as_deref(), b.crop.as_deref()) {
        planned.crops = Some((crop_a, crop_b));
        match (a.instances.first(), b.instances.first()) {
            (Some(x), Some(y))
                if (x.box_.w - y.box_.w).abs() > SIZE_TOLERANCE || (x.box_.h - y.box_.h).abs() > SIZE_TOLERANCE =>
            {
                Verdict::SizeChanged(format!(
                    "box {}x{} became {}x{}",
                    x.box_.w.round(),
                    x.box_.h.round(),
                    y.box_.w.round(),
                    y.box_.h.round()
                ))
            }
            _ => Verdict::Compare,
        }
    } else {
        skip("no_crop")
    };
    planned
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use traceqa_diff::{compare, Options};

    fn component(id: &str, edit: impl FnOnce(&mut Value)) -> Component {
        let mut value = json!({
            "id": id, "role": "button", "name": "Buy", "nameIsTemplate": false, "confidence": "high",
            "signals": [], "path": "main", "crop": format!("crops/{id}.png"),
            "instances": [{
                "key": format!("{id}:0"), "name": "Buy", "tag": "button", "attrs": {}, "selectors": ["button"],
                "box": {"x": 0, "y": 0, "w": 100, "h": 30},
                "state": {"visible": true, "enabled": true, "inViewport": true},
                "frameId": "main", "inShadowDom": false
            }]
        });
        edit(&mut value);
        serde_json::from_value(value).expect("a valid component")
    }

    #[allow(clippy::needless_pass_by_value)]
    fn file(components: Vec<Component>) -> ComponentsFile {
        serde_json::from_value(json!({
            "formatVersion": 1, "fingerprintVersion": 1, "runId": "r", "url": "http://x/", "finalUrl": "http://x/",
            "title": "t", "browser": {"name": "chromium", "version": "156.0.1"},
            "viewport": {"width": 1280, "height": 800}, "capturedAt": "2026-01-01T00:00:00Z",
            "components": components, "warnings": []
        }))
        .expect("a valid file")
    }

    /// Plans the first component of each side and returns its verdict.
    fn verdict(a: Component, b: Component) -> Verdict {
        let (a, b) = (file(vec![a]), file(vec![b]));
        let diff = compare(&a, &b, &Options::default()).expect("same fingerprint version");
        plan(&a, &b, &diff).remove(0).verdict
    }

    fn plain(edit: impl FnOnce(&mut Value)) -> Component {
        component("c_aaaaaaaaaaaa", edit)
    }

    #[test]
    fn an_unchanged_component_is_compared() {
        assert_eq!(verdict(plain(|_| {}), plain(|_| {})), Verdict::Compare);
    }

    #[test]
    fn components_that_cannot_be_compared_say_why() {
        let reason = |a: Component, b: Component| match verdict(a, b) {
            Verdict::Skip { reason, .. } => reason,
            other => panic!("expected a skip, got {other:?}"),
        };
        let covered = plain(|c| c["instances"][0]["state"]["covered"] = json!(true));
        assert_eq!(reason(covered, plain(|_| {})), "covered");
        let hidden = plain(|c| c["instances"][0]["state"]["visible"] = json!(false));
        assert_eq!(reason(plain(|_| {}), hidden), "not_visible");
        let no_crop = plain(|c| {
            c.as_object_mut().expect("object").remove("crop");
        });
        assert_eq!(reason(plain(|_| {}), no_crop), "no_crop");
        let templated = plain(|c| c["nameIsTemplate"] = json!(true));
        assert_eq!(reason(templated.clone(), templated), "name_is_template");
        let moving = plain(|c| c["nameChanges"] = json!(true));
        assert_eq!(reason(plain(|_| {}), moving), "name_changes");
    }

    #[test]
    fn a_structural_change_is_skipped_with_the_fields() {
        let disabled = plain(|c| c["instances"][0]["state"]["enabled"] = json!(false));
        match verdict(plain(|_| {}), disabled) {
            Verdict::Skip { reason, detail } => {
                assert_eq!(reason, "structurally_changed");
                assert_eq!(detail.as_deref(), Some("changed: enabled"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_resized_box_is_size_changed_but_one_pixel_is_tolerated() {
        let wider = plain(|c| c["instances"][0]["box"]["w"] = json!(120));
        assert!(matches!(verdict(plain(|_| {}), wider), Verdict::SizeChanged(d) if d == "box 100x30 became 120x30"));
        let nudged = plain(|c| c["instances"][0]["box"]["w"] = json!(100.9));
        assert_eq!(verdict(plain(|_| {}), nudged), Verdict::Compare);
    }

    #[test]
    fn components_on_one_side_are_skipped_and_results_are_ordered_by_id() {
        let a = file(vec![
            component("c_bbbbbbbbbbbb", |_| {}),
            component("c_aaaaaaaaaaaa", |_| {}),
        ]);
        let b = file(vec![
            component("c_cccccccccccc", |_| {}),
            component("c_aaaaaaaaaaaa", |_| {}),
        ]);
        let diff = compare(&a, &b, &Options::default()).expect("same fingerprint version");
        let planned = plan(&a, &b, &diff);
        let summary: Vec<(&str, String)> = planned.iter().map(|p| (p.id, format!("{:?}", p.verdict))).collect();
        assert_eq!(summary[0], ("c_aaaaaaaaaaaa", "Compare".to_string()));
        assert!(summary[1].1.contains("removed") && summary[1].0 == "c_bbbbbbbbbbbb");
        assert!(summary[2].1.contains("added") && summary[2].0 == "c_cccccccccccc");
    }
}
