//! Tests built from small synthetic files, so each case shows one rule.

use serde_json::{json, Value};
use traceqa_protocol::types::ComponentsFile;

use crate::*;

fn instance(id: &str, n: usize, state: &Value) -> Value {
    json!({
        "key": format!("{id}:{n}"), "name": "x", "tag": "button", "attrs": {}, "selectors": [],
        "box": {"x": 1, "y": 2, "w": 3, "h": 4}, "state": state, "frameId": "main", "inShadowDom": false
    })
}

/// A component with `count` instances that all have `state` merged over the default state.
fn component(id: &str, role: &str, name: &str, path: &str, count: usize, state: &Value) -> Value {
    let mut merged = json!({"visible": true, "enabled": true, "inViewport": true});
    for (k, v) in state.as_object().unwrap() {
        merged[k] = v.clone();
    }
    json!({
        "id": id, "role": role, "name": name, "nameIsTemplate": name == "*", "confidence": "high",
        "signals": ["native"], "path": path,
        "instances": (0..count).map(|n| instance(id, n, &merged)).collect::<Vec<_>>()
    })
}

fn plain(id: &str, name: &str) -> Value {
    component(id, "button", name, "main", 1, &json!({}))
}

fn file_with(run: &str, browser: &str, width: u64, version: u64, components: &[Value]) -> ComponentsFile {
    serde_json::from_value(json!({
        "formatVersion": 1, "fingerprintVersion": version, "runId": run, "url": "http://x/", "finalUrl": "http://x/",
        "title": "t", "browser": {"name": browser, "version": "1"}, "viewport": {"width": width, "height": 800},
        "capturedAt": "2026-01-01T00:00:00Z", "components": components, "warnings": []
    }))
    .unwrap()
}

fn file(components: &[Value]) -> ComponentsFile {
    file_with("r_1", "chromium", 1280, 2, components)
}

fn diff(a: &[Value], b: &[Value]) -> DiffFile {
    compare(&file(a), &file(b), &Options::default()).unwrap()
}

fn only(d: &DiffFile) -> &DiffEntry {
    assert_eq!(d.entries.len(), 1, "{:#?}", d.entries);
    &d.entries[0]
}

#[test]
fn identical_files_give_no_entries() {
    let c = [plain("c_000000000001", "Buy"), plain("c_000000000002", "Sell")];
    let d = diff(&c, &c);
    assert!(d.entries.is_empty());
    assert_eq!(
        (
            d.summary.unchanged,
            d.summary.added,
            d.summary.removed,
            d.summary.changed
        ),
        (2, 0, 0, 0)
    );
}

#[test]
fn an_added_component_is_in_b_only() {
    let d = diff(
        &[plain("c_000000000001", "Buy")],
        &[plain("c_000000000001", "Buy"), plain("c_000000000002", "Sell")],
    );
    let e = only(&d);
    assert_eq!((e.kind, e.id.as_str()), (DiffEntryKind::Added, "c_000000000002"));
    assert!(e.a.is_none() && e.b.is_some());
    assert_eq!(d.summary.added, 1);
}

#[test]
fn a_removed_component_is_in_a_only() {
    let d = diff(
        &[plain("c_000000000001", "Buy"), plain("c_000000000002", "Sell")],
        &[plain("c_000000000001", "Buy")],
    );
    let e = only(&d);
    assert_eq!((e.kind, e.id.as_str()), (DiffEntryKind::Removed, "c_000000000002"));
    assert!(e.a.is_some() && e.b.is_none());
}

#[test]
fn an_instance_count_change_is_flagged() {
    let row = |n| component("c_000000000001", "button", "Add", "list", n, &json!({}));
    let d = diff(&[row(12)], &[row(11)]);
    let e = only(&d);
    assert_eq!(e.kind, DiffEntryKind::Changed);
    assert_eq!(e.fields, ["instances", "visible", "enabled"]);
    assert_eq!(e.flags, ["count_down"]);
    let d = diff(&[row(11)], &[row(12)]);
    assert_eq!(only(&d).flags, ["count_up"]);
}

#[test]
fn losing_every_visible_instance_is_became_hidden() {
    let shown = component("c_000000000001", "button", "Menu", "nav", 3, &json!({}));
    let hidden = component("c_000000000001", "button", "Menu", "nav", 3, &json!({"visible": false}));
    let e = diff(std::slice::from_ref(&shown), std::slice::from_ref(&hidden));
    assert_eq!(only(&e).fields, ["visible"]);
    assert_eq!(only(&e).flags, ["became_hidden"]);
    let e = diff(&[hidden], &[shown]);
    assert_eq!(only(&e).flags, ["became_visible"]);
}

#[test]
fn a_hidden_component_is_still_a_component() {
    // visible 0 is a state, not a removal.
    let hidden = component("c_000000000001", "button", "Menu", "nav", 1, &json!({"visible": false}));
    let d = diff(std::slice::from_ref(&hidden), std::slice::from_ref(&hidden));
    assert!(d.entries.is_empty());
    assert_eq!(d.summary.unchanged, 1);
}

#[test]
fn losing_every_enabled_instance_is_became_disabled() {
    let on = component("c_000000000001", "button", "Pay", "form", 1, &json!({}));
    let off = component("c_000000000001", "button", "Pay", "form", 1, &json!({"enabled": false}));
    let d = diff(std::slice::from_ref(&on), std::slice::from_ref(&off));
    assert_eq!(only(&d).flags, ["became_disabled"]);
    let d = diff(&[off], &[on]);
    assert_eq!(only(&d).flags, ["became_enabled"]);
}

#[test]
fn covered_flips_are_flagged() {
    let free = component("c_000000000001", "button", "Pay", "form", 1, &json!({"covered": false}));
    let blocked = component("c_000000000001", "button", "Pay", "form", 1, &json!({"covered": true}));
    let d = diff(std::slice::from_ref(&free), std::slice::from_ref(&blocked));
    assert_eq!(only(&d).fields, ["covered"]);
    assert_eq!(only(&d).flags, ["became_covered"]);
    let d = diff(&[blocked], &[free]);
    assert_eq!(only(&d).flags, ["became_uncovered"]);
}

#[test]
fn other_state_counts_and_confidence_are_compared() {
    let base = component("c_000000000001", "checkbox", "Agree", "form", 1, &json!({}));
    let checked = component(
        "c_000000000001",
        "checkbox",
        "Agree",
        "form",
        1,
        &json!({"checked": true}),
    );
    let d = diff(std::slice::from_ref(&base), &[checked]);
    assert_eq!(only(&d).fields, ["checked"]);
    assert!(only(&d).flags.is_empty());
    let mut low = base.clone();
    low["confidence"] = json!("low");
    let d = diff(&[base], &[low]);
    assert_eq!(only(&d).fields, ["confidence"]);
}

#[test]
fn boxes_are_ignored() {
    let a = component("c_000000000001", "button", "Buy", "main", 1, &json!({}));
    let mut b = a.clone();
    b["instances"][0]["box"] = json!({"x": 900, "y": 900, "w": 5, "h": 5});
    assert!(diff(&[a], &[b]).entries.is_empty());
}

#[test]
fn a_role_change_is_a_paired_removal_and_addition() {
    let a = component("c_000000000001", "button", "Buy", "main", 1, &json!({}));
    let b = component("c_000000000002", "link", "Buy", "main", 1, &json!({}));
    let d = diff(&[a], &[b]);
    assert_eq!(
        (d.summary.removed, d.summary.added, d.summary.changed, d.summary.paired),
        (1, 1, 0, 1)
    );
    let (removed, added) = (&d.entries[0], &d.entries[1]);
    assert_eq!(
        (removed.kind, added.kind),
        (DiffEntryKind::Removed, DiffEntryKind::Added)
    );
    let link = removed.paired_with.as_ref().unwrap();
    assert_eq!(
        (link.id.as_str(), link.reason.as_str()),
        ("c_000000000002", "role_changed")
    );
    assert_eq!(link.confidence.to_string(), "high");
    assert_eq!(added.paired_with.as_ref().unwrap().id, "c_000000000001");
}

#[test]
fn a_unique_rename_pairs_with_low_confidence() {
    let d = diff(
        &[plain("c_000000000001", "Buy")],
        &[plain("c_000000000002", "Purchase")],
    );
    let link = d.entries[0].paired_with.as_ref().unwrap();
    assert_eq!(
        (link.reason.as_str(), link.confidence.to_string()),
        ("name_changed", "low".to_string())
    );
}

#[test]
fn two_renames_in_one_bucket_pair_nothing() {
    let a = [plain("c_000000000001", "A"), plain("c_000000000002", "B")];
    let b = [plain("c_000000000003", "C"), plain("c_000000000004", "D")];
    let d = diff(&a, &b);
    assert_eq!((d.summary.removed, d.summary.added, d.summary.paired), (2, 2, 0));
    assert!(d.entries.iter().all(|e| e.paired_with.is_none()));
}

#[test]
fn entries_are_ordered_the_same_whatever_the_input_order() {
    let a = [
        plain("c_000000000001", "Zed"),
        plain("c_000000000002", "Amy"),
        component("c_000000000003", "button", "Same", "main", 2, &json!({})),
        plain("c_000000000009", "Gone"),
    ];
    let b = [
        plain("c_000000000004", "Mid"),
        component("c_000000000003", "button", "Same", "main", 1, &json!({})),
        plain("c_000000000001", "Zed"),
        plain("c_000000000005", "New"),
        plain("c_000000000002", "Amy"),
    ];
    let expected = diff(&a, &b);
    let kinds: Vec<_> = expected.entries.iter().map(|e| (e.kind, e.id.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (DiffEntryKind::Removed, "c_000000000009"),
            (DiffEntryKind::Added, "c_000000000004"),
            (DiffEntryKind::Added, "c_000000000005"),
            (DiffEntryKind::Changed, "c_000000000003"),
        ]
    );
    for (ra, rb) in [(true, false), (false, true), (true, true)] {
        let (mut a2, mut b2) = (a.to_vec(), b.to_vec());
        if ra {
            a2.reverse();
        }
        if rb {
            b2.reverse();
        }
        assert_eq!(diff(&a2, &b2), expected);
    }
}

#[test]
fn different_fingerprint_versions_are_an_error() {
    let a = file_with("r_1", "chromium", 1280, 1, &[]);
    let b = file_with("r_2", "chromium", 1280, 2, &[]);
    let error = compare(&a, &b, &Options::default()).unwrap_err();
    assert_eq!(error, DiffError::FingerprintVersion { a: 1, b: 2 });
    assert!(error.to_string().contains("fingerprint versions (1 and 2)"));
}

#[test]
fn the_mode_is_worked_out_from_the_sides() {
    let c = [plain("c_000000000001", "Buy")];
    let mode = |a: ComponentsFile, b: ComponentsFile| compare(&a, &b, &Options::default()).unwrap().mode;
    assert_eq!(
        mode(
            file_with("r_1", "chromium", 1280, 2, &c),
            file_with("r_1", "firefox", 1280, 2, &c)
        ),
        "cross-browser"
    );
    assert_eq!(
        mode(
            file_with("r_1", "chromium", 1280, 2, &c),
            file_with("r_1", "chromium", 390, 2, &c)
        ),
        "cross-viewport"
    );
    assert_eq!(
        mode(
            file_with("r_1", "chromium", 1280, 2, &c),
            file_with("r_2", "chromium", 1280, 2, &c)
        ),
        "run-to-run"
    );
    assert_eq!(
        mode(
            file_with("r_1", "chromium", 1280, 2, &c),
            file_with("r_2", "firefox", 390, 2, &c)
        ),
        "custom"
    );
}

#[test]
fn a_viewport_difference_adds_a_note() {
    let c = [plain("c_000000000001", "Buy")];
    let d = compare(
        &file_with("r_1", "chromium", 1280, 2, &c),
        &file_with("r_1", "chromium", 390, 2, &c),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(d.warnings[0].code, "viewports_differ");
    assert!(diff(&c, &c).warnings.is_empty());
}

#[test]
fn pages_that_share_few_components_are_said_to() {
    let a: Vec<Value> = (1..=4)
        .map(|n| plain(&format!("c_00000000000{n}"), &format!("A{n}")))
        .collect();
    let b: Vec<Value> = (5..=8)
        .map(|n| plain(&format!("c_00000000000{n}"), &format!("B{n}")))
        .collect();
    let d = diff(&a, &b);
    assert_eq!(d.warnings[0].code, "few_shared_components");
}

#[test]
fn the_diff_serialises_to_the_same_bytes_every_time() {
    let a = [plain("c_000000000001", "Buy"), plain("c_000000000002", "Old")];
    let b = [plain("c_000000000001", "Buy"), plain("c_000000000003", "New")];
    let first = serde_json::to_string_pretty(&diff(&a, &b)).unwrap();
    assert_eq!(first, serde_json::to_string_pretty(&diff(&a, &b)).unwrap());
    assert!(first.contains("\"pairedWith\""));
}

// ---- Markdown ----

#[test]
fn an_empty_diff_says_so() {
    let c = [plain("c_000000000001", "Buy")];
    let md = render_markdown(&diff(&c, &c));
    assert!(md.contains("0 removed, 0 added, 0 changed, 1 unchanged, 0 paired"));
    assert!(md.contains("No differences."));
}

#[test]
fn a_hostile_name_renders_safely() {
    let nasty = format!(
        "a`b``c|d\ne\u{202E}f @everyone <img src=x onerror=1> {}",
        "z".repeat(500)
    );
    let d = diff(&[], &[plain("c_000000000001", &nasty)]);
    let md = render_markdown(&d);
    let row = md.lines().find(|l| l.starts_with("| `c_000000000001`")).expect("a row");
    assert_eq!(row.matches(" | ").count(), 4, "the row stays five cells: {row}");
    assert!(!row.contains('\u{202E}') && !row.contains('\n'));
    // The name is in a fence of three backticks, one longer than the longest run (two).
    assert!(row.contains("```a`b``c"), "{row}");
    assert!(row.contains('\u{2026}'), "cut to 80 characters: {row}");
    assert!(row.chars().count() < 250, "{row}");
    // Page text only ever appears inside a code span: remove the spans and no mention is left.
    let outside = row.replace(&row[row.find("```").unwrap()..row.rfind("```").unwrap() + 3], "");
    assert!(!outside.contains('@') && !outside.contains('<'), "{outside}");
}

#[test]
fn each_table_is_cut_at_fifty_rows() {
    let many: Vec<Value> = (0..120)
        .map(|n| plain(&format!("c_{n:012x}"), &format!("Button {n}")))
        .collect();
    let d = diff(&[], &many);
    let md = render_markdown(&d);
    assert_eq!(md.lines().filter(|l| l.starts_with("| `c_")).count(), MAX_ROWS);
    assert!(md.contains("and 70 more, use --format json"));
    assert!(md.len() < 64 * 1024, "{}", md.len());
    // The JSON keeps every entry.
    assert_eq!(d.entries.len(), 120);
}

#[test]
fn changes_and_pairs_show_in_the_tables() {
    let a = [
        component("c_000000000001", "button", "Add", "list", 12, &json!({})),
        component("c_000000000002", "button", "Buy", "main", 1, &json!({})),
    ];
    let b = [
        component("c_000000000001", "button", "Add", "list", 11, &json!({})),
        component("c_000000000003", "link", "Buy", "main", 1, &json!({})),
    ];
    let md = render_markdown(&diff(&a, &b));
    assert!(md.contains("### Removed (in A only) (1)") && md.contains("### Added (in B only) (1)"));
    assert!(
        md.contains("instances 12 \u{2192} 11; visible 12 \u{2192} 11; enabled 12 \u{2192} 11"),
        "{md}"
    );
    assert!(md.contains("`c_000000000003` (role_changed, high confidence)"), "{md}");
}
