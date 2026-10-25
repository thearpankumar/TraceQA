use serde_json::{json, Value};
use traceqa_protocol::types::{BaselineAlias, ComponentsFile};

use crate::*;

const SHOP: &str = include_str!("../../../spec/examples/ComponentsFile.shop.json");

struct C {
    id: &'static str,
    role: &'static str,
    name: &'static str,
    path: &'static str,
    test_id: Option<&'static str>,
    at: (f64, f64),
    locator_test_id: Option<&'static str>,
}

fn c(id: &'static str, role: &'static str, name: &'static str) -> C {
    C {
        id,
        role,
        name,
        path: "main",
        test_id: None,
        at: (100.0, 100.0),
        locator_test_id: None,
    }
}

fn file(components: &[C]) -> ComponentsFile {
    let mut base: Value = serde_json::from_str(SHOP).unwrap();
    base["components"] = Value::Array(
        components
            .iter()
            .map(|c| {
                let mut attrs = serde_json::Map::new();
                if let Some(t) = c.test_id {
                    attrs.insert("data-testid".into(), json!(t));
                }
                let locator = match c.locator_test_id {
                    Some(t) => json!({"strategy": "testid", "testId": t, "unique": true, "matches": 1, "own": 1, "forced": false, "needsIncludeHidden": false}),
                    None => json!({"strategy": "role", "role": c.role, "name": c.name, "exact": true, "unique": true, "matches": 1, "own": 1, "forced": false, "needsIncludeHidden": false}),
                };
                json!({
                    "id": c.id, "role": c.role, "name": c.name, "nameIsTemplate": false,
                    "confidence": "high", "signals": [], "path": c.path, "locator": locator,
                    "instances": [{
                        "key": format!("{}:0", c.id), "name": c.name, "tag": "button", "attrs": attrs,
                        "selectors": [], "box": {"x": c.at.0, "y": c.at.1, "w": 80.0, "h": 30.0},
                        "state": {"visible": true, "enabled": true, "inViewport": true},
                        "frameId": "main", "inShadowDom": false
                    }]
                })
            })
            .collect(),
    );
    base["warnings"] = json!([]);
    base["viewport"] = json!({"width": 1000, "height": 800});
    serde_json::from_value(base).unwrap()
}

fn heal(baseline: &[C], run: &[C]) -> Vec<HealItem> {
    classify(&file(baseline), &file(run), &[], "chromium-1000x800").unwrap()
}

fn only(items: Vec<HealItem>) -> HealItem {
    assert_eq!(items.len(), 1, "{items:#?}");
    items.into_iter().next().unwrap()
}

#[test]
fn the_same_id_with_the_same_locator_is_none() {
    let item = only(heal(
        &[c("c_000000000001", "button", "Buy")],
        &[c("c_000000000001", "button", "Buy")],
    ));
    assert_eq!((item.kind, item.action), (HealKind::Unchanged, HealAction::None));
}

#[test]
fn the_same_id_with_another_locator_is_a_mechanical_fix() {
    let mut now = c("c_000000000001", "button", "Buy");
    now.locator_test_id = Some("buy");
    let item = only(heal(&[c("c_000000000001", "button", "Buy")], &[now]));
    assert_eq!((item.kind, item.action), (HealKind::LocatorFix, HealAction::Fix));
    assert_eq!(item.new_id.as_deref(), Some("c_000000000001"));
    let preview = item.fix_preview.unwrap();
    assert!(preview.starts_with("- page.getByRole(\"button\""), "{preview}");
    assert!(preview.contains("\n+ page.getByTestId(\"buy\")"), "{preview}");
}

#[test]
fn a_rename_with_the_same_test_id_is_high_confidence() {
    let mut old = c("c_000000000001", "button", "Buy");
    old.test_id = Some("buy-btn");
    let mut new = c("c_000000000002", "button", "Purchase now");
    new.test_id = Some("buy-btn");
    new.at = (900.0, 700.0); // far away: the test id alone is the evidence
    let item = only(heal(&[old], &[new]));
    assert_eq!(
        (item.kind, item.action),
        (HealKind::RenameProposal, HealAction::Propose)
    );
    assert_eq!(item.confidence, Some(HealConfidence::High));
    assert_eq!(item.new_id.as_deref(), Some("c_000000000002"));
    assert!(item.evidence.iter().any(|e| e.contains("same data-testid")));
}

#[test]
fn a_rename_by_name_alone_is_medium_confidence() {
    let item = only(heal(
        &[c("c_000000000001", "button", "Add to cart")],
        &[c("c_000000000002", "button", "Add to carts")],
    ));
    assert_eq!(item.kind, HealKind::RenameProposal);
    assert_eq!(item.confidence, Some(HealConfidence::Medium));
    assert!(item.evidence.iter().any(|e| e.contains("similarity")));
}

#[test]
fn two_similar_renamed_buttons_are_ambiguous_and_nothing_is_proposed() {
    let items = heal(
        &[
            c("c_000000000001", "button", "Save draft"),
            c("c_000000000002", "button", "Save drafts"),
        ],
        &[
            c("c_000000000003", "button", "Save draft!"),
            c("c_000000000004", "button", "Save drafts!"),
        ],
    );
    assert_eq!(items.len(), 2);
    for item in &items {
        assert_eq!(
            (item.kind, item.action),
            (HealKind::Ambiguous, HealAction::Refuse),
            "{item:#?}"
        );
        assert!(item.new_id.is_none());
        assert!(item.evidence.iter().any(|e| e.contains("candidate c_")));
    }
}

#[test]
fn two_removed_for_one_added_is_ambiguous_on_both() {
    let items = heal(
        &[
            c("c_000000000001", "button", "Save draft"),
            c("c_000000000002", "button", "Save drafts"),
        ],
        &[c("c_000000000003", "button", "Save draft")],
    );
    assert!(items.iter().all(|i| i.kind == HealKind::Ambiguous), "{items:#?}");
}

#[test]
fn a_removal_is_a_failure() {
    let item = only(heal(&[c("c_000000000001", "button", "Buy")], &[]));
    assert_eq!((item.kind, item.action), (HealKind::Removed, HealAction::Fail));
}

#[test]
fn an_unrelated_addition_does_not_save_a_removal() {
    let item = only(heal(
        &[c("c_000000000001", "button", "Buy")],
        &[c("c_000000000002", "button", "Subscribe to our newsletter")],
    ));
    assert_eq!(item.kind, HealKind::Removed);
}

#[test]
fn a_different_role_is_never_a_candidate() {
    let mut old = c("c_000000000001", "button", "Buy");
    old.test_id = Some("buy");
    let mut new = c("c_000000000002", "link", "Buy");
    new.test_id = Some("buy");
    assert_eq!(only(heal(&[old], &[new])).kind, HealKind::Removed);
}

#[test]
fn another_landmark_is_never_a_candidate() {
    let old = c("c_000000000001", "button", "Buy");
    let mut new = c("c_000000000002", "button", "Buy");
    new.path = "navigation \"menu\"";
    assert_eq!(only(heal(&[old], &[new])).kind, HealKind::Removed);
}

#[test]
fn a_similar_name_far_away_is_rejected() {
    let old = c("c_000000000001", "button", "Add to cart");
    let mut new = c("c_000000000002", "button", "Add to carts");
    new.at = (100.0 + 150.0, 100.0); // 15% of the 1000 px viewport
    assert_eq!(only(heal(&[old], &[new])).kind, HealKind::Removed);
}

#[test]
fn a_small_move_is_accepted() {
    let old = c("c_000000000001", "button", "Add to cart");
    let mut new = c("c_000000000002", "button", "Add to carts");
    new.at = (100.0 + 90.0, 100.0 + 70.0);
    assert_eq!(only(heal(&[old], &[new])).kind, HealKind::RenameProposal);
}

#[test]
fn a_baseline_alias_accounts_for_a_rename() {
    let alias = BaselineAlias {
        from: "c_000000000001".into(),
        to: "c_000000000002".into(),
        reason: "heal".into(),
        evidence: None,
        added_at: "2026-01-01T00:00:00Z".into(),
    };
    let items = classify(
        &file(&[c("c_000000000001", "button", "Add to cart")]),
        &file(&[c("c_000000000002", "button", "Add to carts")]),
        &[alias],
        "t",
    )
    .unwrap();
    assert_eq!(only(items).kind, HealKind::Unchanged);
}

#[test]
fn a_signed_out_run_short_circuits_everything() {
    let mut run = file(&[]);
    run.warnings.push(traceqa_protocol::types::Warning {
        code: SESSION_NOT_ACCEPTED.into(),
        message: "sign in again".into(),
    });
    let items = classify(&file(&[c("c_000000000001", "button", "Buy")]), &run, &[], "t").unwrap();
    let item = only(items);
    assert_eq!(
        (item.kind, item.action),
        (HealKind::SessionNotAccepted, HealAction::Refuse)
    );
}

#[test]
fn ids_of_other_recipes_are_refused() {
    let mut run = file(&[]);
    run.fingerprint_version += 1;
    let err = classify(&file(&[]), &run, &[], "t").unwrap_err();
    assert!(matches!(err, HealError::FingerprintVersion { .. }));
}

#[test]
fn the_landmark_is_the_first_landmark_step() {
    assert_eq!(landmark("div > main > list \"x\" > listitem"), "main");
    assert_eq!(
        landmark("div > navigation \"Contents\" > link"),
        "navigation \"Contents\""
    );
    assert_eq!(landmark("div > list"), "");
}

fn baseline_ref() -> traceqa_protocol::types::BaselineRef {
    traceqa_protocol::types::BaselineRef {
        key: "shop".into(),
        url: "http://x/".into(),
        promoted_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn the_report_counts_unchanged_and_fails_on_a_proposal_until_it_is_accepted() {
    let mut items = heal(
        &[
            c("c_000000000001", "button", "Add to cart"),
            c("c_000000000009", "button", "Home"),
        ],
        &[
            c("c_000000000002", "button", "Add to carts"),
            c("c_000000000009", "button", "Home"),
        ],
    );
    let report = build_report("r_1", baseline_ref(), items.clone());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.summary.unchanged, 1);
    assert!(report.summary.failed);
    for item in &mut items {
        item.accepted = Some(true);
    }
    let report = build_report("r_1", baseline_ref(), items);
    assert!(!report.summary.failed);
    assert_eq!(report.summary.accepted, Some(1));
}

#[test]
fn a_fix_alone_does_not_fail_the_report() {
    let mut now = c("c_000000000001", "button", "Buy");
    now.locator_test_id = Some("buy");
    let report = build_report(
        "r",
        baseline_ref(),
        heal(&[c("c_000000000001", "button", "Buy")], &[now]),
    );
    assert!(!report.summary.failed);
    assert_eq!(report.summary.locator_fixes, 1);
}

#[test]
fn markdown_shows_page_text_in_code_spans() {
    let report = build_report(
        "r",
        baseline_ref(),
        heal(&[c("c_000000000001", "button", "[click](http://evil)")], &[]),
    );
    let text = render_markdown(&report);
    assert!(text.contains("### Removed `c_000000000001`"), "{text}");
    assert!(text.contains("`button` `[click](http://evil)`"), "{text}");
}
