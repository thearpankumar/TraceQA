//! Tests of `locator.rs`, run through `group()` so the hook in the grouping is covered too.

use std::collections::HashMap;

use traceqa_protocol::types::{Component, Confidence, ElementState, InteractiveEvidence, PathStep, RawElement, Rect};

use super::*;
use crate::group::{group, Located};

fn step(role: &str, name: Option<&str>, repeat: Option<(u64, u64)>) -> PathStep {
    PathStep {
        role: role.into(),
        tag: "div".into(),
        name: name.map(Into::into),
        repeat_index: repeat.map(|r| r.0),
        repeat_count: repeat.map(|r| r.1),
    }
}

fn element(role: &str, name: &str, path: Vec<PathStep>) -> Located {
    Located {
        element: RawElement {
            role: role.into(),
            name: name.into(),
            name_source: None,
            tag: "button".into(),
            attrs: HashMap::new(),
            text: None,
            selectors: vec![],
            box_: Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            state: ElementState {
                visible: true,
                enabled: true,
                in_viewport: true,
                checked: None,
                expanded: None,
                selected: None,
                pressed: None,
                readonly: None,
                required: None,
                covered: None,
                covered_by: None,
            },
            in_shadow_dom: false,
            path,
            interactive: InteractiveEvidence {
                confidence: Confidence::High,
                signals: vec!["native".into()],
            },
        },
        frame_id: "main".into(),
        frame_step: None,
        page_box: None,
        name_changes: false,
    }
}

fn with_selector(mut located: Located, selector: &str) -> Located {
    located.element.selectors = vec![selector.into()];
    located
}

fn with_test_id(mut located: Located, id: &str) -> Located {
    located.element.attrs.insert("data-testid".into(), id.into());
    located
}

fn named<'a>(components: &'a [Component], role: &str, name: &str) -> &'a Locator {
    components
        .iter()
        .find(|c| c.role == role && c.name == name)
        .and_then(|c| c.locator.as_ref())
        .expect("a locator for the component")
}

#[test]
fn a_role_and_name_that_is_alone_is_unique() {
    let components = group(vec![
        element("button", "Pay now", vec![step("main", None, None)]),
        element("link", "Home", vec![step("banner", None, None)]),
    ]);
    let locator = named(&components, "button", "Pay now");
    assert_eq!(locator.strategy, LocatorStrategy::Role);
    assert_eq!(locator.role.as_deref(), Some("button"));
    assert_eq!(locator.name.as_deref(), Some("Pay now"));
    assert_eq!(locator.exact, Some(true));
    assert!(locator.scope.is_empty());
    assert!(locator.unique && !locator.forced && !locator.needs_include_hidden);
    assert_eq!((locator.matches, locator.own), (1, 1));
}

#[test]
fn the_same_role_and_name_in_two_places_gets_a_scope_chain() {
    let components = group(vec![
        element("link", "Home", vec![step("navigation", Some("main"), None)]),
        element("link", "Home", vec![step("contentinfo", None, None)]),
    ]);
    assert_eq!(components.len(), 2);
    let scopes: Vec<_> = components
        .iter()
        .map(|c| {
            let locator = c.locator.as_ref().expect("locator");
            assert!(locator.unique, "{locator:?}");
            assert_eq!((locator.matches, locator.own), (1, 1));
            locator
                .scope
                .iter()
                .map(|s| (s.role.clone(), s.name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(scopes[0], [("navigation".to_string(), Some("main".to_string()))]);
    assert_eq!(scopes[1], [("contentinfo".to_string(), None)]);
}

#[test]
fn a_name_that_is_part_of_another_name_stays_unique_because_it_is_exact() {
    // Without `exact`, getByRole("button", {name: "Add"}) would also find "Add to cart".
    let components = group(vec![
        element("button", "Add", vec![step("main", None, None)]),
        element("button", "Add to cart", vec![step("main", None, None)]),
    ]);
    for name in ["Add", "Add to cart"] {
        let locator = named(&components, "button", name);
        assert!(locator.unique && locator.scope.is_empty(), "{name}: {locator:?}");
        assert_eq!(locator.exact, Some(true));
    }
}

#[test]
fn matching_is_case_sensitive_so_a_locator_that_misses_an_instance_is_not_unique() {
    // "Save" and "SAVE" are one component (ids ignore case) but an exact name finds one of them.
    let components = group(vec![
        with_selector(element("button", "Save", vec![step("main", None, None)]), "#a"),
        with_selector(element("button", "SAVE", vec![step("main", None, None)]), "#b"),
    ]);
    assert_eq!(components.len(), 1);
    let locator = components[0].locator.as_ref().expect("locator");
    assert_eq!(locator.strategy, LocatorStrategy::Css);
    assert!(locator.forced);
    assert_eq!(locator.css.as_deref(), Some("#a, #b"));
}

#[test]
fn a_templated_name_is_found_by_role_inside_its_list() {
    let mut elements: Vec<Located> = (0..4)
        .flat_map(|i| {
            let path = vec![
                step("list", Some("products"), None),
                step("listitem", None, Some((i, 4))),
            ];
            [
                element("link", &format!("Item {i}"), path.clone()),
                element("button", "Add", path),
            ]
        })
        .collect();
    elements.push(element("link", "Home", vec![step("banner", None, None)]));
    let components = group(elements);
    let links = components
        .iter()
        .find(|c| c.name_is_template && c.role == "link")
        .and_then(|c| c.locator.as_ref())
        .expect("templated link");
    assert_eq!(links.strategy, LocatorStrategy::Role);
    assert_eq!(links.name, None);
    assert_eq!(links.exact, None);
    assert_eq!(links.scope.len(), 1);
    assert_eq!(links.scope[0].role, "list");
    assert!(links.unique, "{links:?}");
    assert_eq!((links.matches, links.own), (4, 4));
    // The button repeats in each card but is one control: one locator, four matches.
    let add = named(&components, "button", "Add");
    assert!(add.unique);
    assert_eq!((add.matches, add.own), (4, 4));
}

#[test]
fn a_test_id_shared_by_every_instance_is_used_when_there_is_no_role_and_name() {
    let components = group(vec![
        with_test_id(element("generic", "", vec![step("main", None, None)]), "tile"),
        with_test_id(element("generic", "", vec![step("main", None, None)]), "tile"),
    ]);
    let locator = components[0].locator.as_ref().expect("locator");
    assert_eq!(locator.strategy, LocatorStrategy::Testid);
    assert_eq!(locator.test_id.as_deref(), Some("tile"));
    assert!(locator.unique && !locator.forced);
    assert_eq!((locator.matches, locator.own), (2, 2));
}

#[test]
fn a_generic_element_with_a_name_is_found_by_its_text() {
    let mut gift = element("generic", "Gift cards", vec![step("main", None, None)]);
    gift.element.name_source = Some("content".into());
    let components = group(vec![gift]);
    let locator = components[0].locator.as_ref().expect("locator");
    assert_eq!(locator.strategy, LocatorStrategy::Text);
    assert_eq!(locator.name.as_deref(), Some("Gift cards"));
    assert_eq!(locator.exact, Some(true));
    assert!(locator.unique);
}

#[test]
fn a_name_that_is_not_visible_text_is_not_looked_for_as_text() {
    // getByText cannot find an aria-label.
    let mut labelled = element("generic", "Close", vec![step("main", None, None)]);
    labelled.element.name_source = Some("aria-label".into());
    let labelled = with_selector(labelled, "#close");
    let components = group(vec![labelled]);
    let locator = components[0].locator.as_ref().expect("locator");
    assert_eq!(locator.strategy, LocatorStrategy::Css);
    assert!(locator.forced);
}

#[test]
fn roles_the_collector_reports_but_playwright_does_not_see_are_not_looked_up_by_role() {
    // <summary> is reported as a button and a contenteditable <div> as a textbox; getByRole finds neither.
    let mut summary = element("button", "Shipping info", vec![step("main", None, None)]);
    summary.element.tag = "summary".into();
    summary.element.name_source = Some("content".into());
    let mut notes = with_selector(
        element("textbox", "Order notes", vec![step("main", None, None)]),
        "div.notes",
    );
    notes.element.tag = "div".into();
    notes.element.name_source = Some("aria-label".into());
    let components = group(vec![summary, notes]);
    let by_role = |role: &str| {
        components
            .iter()
            .find(|c| c.role == role)
            .and_then(|c| c.locator.as_ref())
            .expect("locator")
    };
    assert_eq!(by_role("button").strategy, LocatorStrategy::Text);
    assert_eq!(by_role("textbox").strategy, LocatorStrategy::Css);
    assert!(by_role("textbox").forced);
}

#[test]
fn an_unnamed_element_is_located_by_css_and_marked_forced() {
    // Two icon buttons side by side: role and scope cannot tell them apart.
    let components = group(vec![
        with_selector(element("button", "", vec![step("banner", None, None)]), "#cart-btn"),
        with_selector(element("button", "", vec![step("banner", None, None)]), "#user-btn"),
    ]);
    let locator = components[0].locator.as_ref().expect("locator");
    assert_eq!(locator.strategy, LocatorStrategy::Css);
    assert_eq!(locator.css.as_deref(), Some("#cart-btn"));
    assert!(locator.forced && locator.unique);
    assert_eq!(locator.exact, None);
}

#[test]
fn shadow_dom_selectors_become_plain_descendant_selectors() {
    let components = group(vec![
        with_selector(element("button", "", vec![]), "#host >>> button.inner"),
        with_selector(element("button", "", vec![]), "#host >>> button.other"),
    ]);
    let locator = components[0].locator.as_ref().expect("locator");
    assert_eq!(locator.css.as_deref(), Some("#host button.inner"));
}

#[test]
fn without_a_selector_an_ambiguous_component_is_reported_not_unique() {
    let components = group(vec![
        element("link", "Home", vec![step("banner", None, None)]),
        element(
            "link",
            "Home",
            vec![step("banner", None, None), step("list", None, None)],
        ),
    ]);
    let first = components[0].locator.as_ref().expect("locator");
    assert!(!first.unique && !first.forced);
    assert_eq!((first.matches, first.own), (2, 1));
    assert_eq!(first.ambiguous_with, [components[1].id.clone()]);
    // The nested one can be told apart by its list.
    assert!(components[1].locator.as_ref().expect("locator").unique);
}

#[test]
fn a_component_that_is_only_hidden_needs_include_hidden() {
    let mut hidden = element("button", "Open menu", vec![step("banner", None, None)]);
    hidden.element.state.visible = false;
    let components = group(vec![
        hidden,
        element("button", "Close", vec![step("banner", None, None)]),
    ]);
    let locator = named(&components, "button", "Open menu");
    assert!(locator.needs_include_hidden && locator.unique);
    assert!(!named(&components, "button", "Close").needs_include_hidden);
}

#[test]
fn a_hidden_twin_does_not_make_a_visible_component_ambiguous() {
    // Playwright does not find hidden elements, so the visible "Close" is alone.
    let mut hidden = element("button", "Close", vec![step("dialog", Some("old"), None)]);
    hidden.element.state.visible = false;
    let components = group(vec![
        hidden,
        element("button", "Close", vec![step("banner", None, None)]),
    ]);
    let visible = components
        .iter()
        .find(|c| c.instances[0].state.visible)
        .and_then(|c| c.locator.as_ref())
        .expect("locator");
    assert!(visible.unique && visible.scope.is_empty(), "{visible:?}");
}

#[test]
fn a_component_in_a_frame_names_the_frame_and_stays_apart_from_the_page() {
    let mut inner = element("button", "Pay now", vec![step("main", None, None)]);
    inner.frame_id = "f2".into();
    inner.frame_step = Some("frame \"pay\"".into());
    let outer = element("button", "Pay now", vec![step("main", None, None)]);
    let components = group(vec![inner, outer]);
    assert_eq!(components.len(), 2);
    let in_frame = components
        .iter()
        .find(|c| c.instances[0].frame_id == "f2")
        .and_then(|c| c.locator.as_ref())
        .expect("locator");
    assert_eq!(in_frame.frame.as_deref(), Some("pay"));
    assert!(in_frame.unique && in_frame.scope.is_empty(), "{in_frame:?}");
    let on_page = components
        .iter()
        .find(|c| c.instances[0].frame_id == "main")
        .and_then(|c| c.locator.as_ref())
        .expect("locator");
    assert_eq!(on_page.frame, None);
    assert!(on_page.unique && on_page.scope.is_empty());
}

#[test]
fn frame_labels_lose_their_path_escapes() {
    assert_eq!(frame_label("frame \"a\\\"b\\\\c\""), "a\"b\\c");
    assert_eq!(frame_label("frame \"/widget/v2\""), "/widget/v2");
}

#[test]
fn ambiguous_with_lists_at_most_five_ids() {
    // Seven links named "Go" under seven different unnamed lists, plus one under none: the last
    // one's role+name and scopes always match the other seven.
    let mut elements: Vec<Located> = (0..7)
        .map(|i| {
            let mut located = element(
                "link",
                "Go",
                vec![
                    step("banner", None, None),
                    step("list", None, None),
                    step("main", Some(&format!("m{i}")), None),
                ],
            );
            located.element.tag = "a".into();
            located
        })
        .collect();
    elements.push(element("link", "Go", vec![step("banner", None, None)]));
    let components = group(elements);
    let last = components.last().and_then(|c| c.locator.as_ref()).expect("locator");
    assert!(!last.unique);
    assert_eq!(last.ambiguous_with.len(), 5);
}
