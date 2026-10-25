//! Turns the flat list of elements into components with stable ids.

use std::collections::{HashMap, HashSet};

use traceqa_common::types::{Component, Confidence, Instance, RawElement, Rect};

use crate::fingerprint::{self, CHANGING, REPEAT_THRESHOLD, TEMPLATE};
use crate::locator::{self, Place};

/// Repeated items under these roles are distinct controls (menu entries, tabs), never one template.
const NEVER_TEMPLATED: [&str; 5] = ["navigation", "menu", "menubar", "tablist", "toolbar"];

/// The cells of a table row repeat, but they are its columns, not items of a list: the
/// repeated item of a table is its row.
const CELLS: [&str; 4] = ["cell", "gridcell", "columnheader", "rowheader"];

/// One element together with where it was found.
pub struct Located {
    pub element: RawElement,
    pub frame_id: String,
    /// First step of the path for elements inside a frame, for example `frame "pay"`.
    pub frame_step: Option<String>,
    /// The element's box in top-level page coordinates, when that is known.
    pub page_box: Option<Rect>,
    /// The name was seen to change while the page was read, so the id leaves it out.
    pub name_changes: bool,
}

/// Identifies "the same control" across the cards of a repeated group: the group it is in,
/// where it sits inside its card, and its position among identical neighbours.
type Slot = (String, String, usize);

struct Analysed {
    path_key: String,
    /// Present when the element is one control of a repeated card and may be templated.
    slot: Option<Slot>,
}

/// Where an element sits relative to its innermost repeated ancestor.
struct Placement {
    /// The repeated group: the frame, which copy of each outer repeat, and the path down to the repeated step.
    container: String,
    /// Which copy of the repeated step the element is inside.
    item: u64,
    /// The element's role and its path below the repeated step.
    inner: String,
}

fn placement(located: &Located) -> Option<Placement> {
    let path = &located.element.path;
    if path.iter().any(|step| NEVER_TEMPLATED.contains(&step.role.as_str())) {
        return None;
    }
    let repeated = path
        .iter()
        .rposition(|step| step.repeat_count.unwrap_or(0) >= REPEAT_THRESHOLD && !CELLS.contains(&step.role.as_str()))?;
    let keys = |steps: &[_]| steps.iter().map(fingerprint::step_key).collect::<Vec<_>>().join(" > ");
    let outer: String = path[..repeated]
        .iter()
        .map(|step| format!("{:?}/", step.repeat_index))
        .collect();
    Some(Placement {
        container: format!("{}\n{outer}\n{}", located.frame_id, keys(&path[..=repeated])),
        item: path[repeated].repeat_index.unwrap_or(0),
        inner: format!("{}\n{}", located.element.role, keys(&path[repeated + 1..])),
    })
}

/// Works out each element's path and whether it is a control of a repeated card.
///
/// Repetition alone is not enough: a footer is a list of different links. A card is an item
/// with at least two controls whose arrangement of controls is shared by enough of its siblings.
fn analyse(elements: &[Located]) -> Vec<Analysed> {
    let placements: Vec<Option<Placement>> = elements.iter().map(placement).collect();

    // The controls of each item, in document order. That list is the item's shape.
    let mut controls: HashMap<(&str, u64), Vec<&str>> = HashMap::new();
    for placed in placements.iter().flatten() {
        controls
            .entry((&placed.container, placed.item))
            .or_default()
            .push(&placed.inner);
    }
    // How many items of each group have each shape.
    let mut items_shaped: HashMap<(&str, &[&str]), u64> = HashMap::new();
    for ((container, _), shape) in &controls {
        *items_shaped.entry((container, shape)).or_default() += 1;
    }

    let mut seen: HashMap<(&str, u64, &str), usize> = HashMap::new();
    elements
        .iter()
        .zip(&placements)
        .map(|(located, placed)| {
            let path_key = fingerprint::path_key(located.frame_step.as_deref(), &located.element.path);
            let slot = placed.as_ref().and_then(|placed| {
                // Every placement was counted above, so both lookups succeed. `?` keeps a refactor
                // that skips one from panicking: the element is then simply no card control.
                let shape = controls.get(&(placed.container.as_str(), placed.item))?;
                let shaped = items_shaped.get(&(placed.container.as_str(), shape.as_slice()))?;
                let is_card = shape.len() >= 2 && *shaped >= REPEAT_THRESHOLD;
                let ordinal = seen.entry((&placed.container, placed.item, &placed.inner)).or_insert(0);
                *ordinal += 1;
                is_card.then(|| (placed.container.clone(), placed.inner.clone(), *ordinal - 1))
            });
            Analysed { path_key, slot }
        })
        .collect()
}

/// The key that stands in for the name when an element has none: something the page
/// states about the element when there is such a thing, else its tag and `position` among
/// the unnamed elements beside it.
fn unnamed_key(element: &RawElement, position: usize) -> String {
    if let Some(test_id) = element.attrs.get("data-testid") {
        return format!("testid:{test_id}");
    }
    // A generated id (":r1:", "radix-12345") differs on every page load, so it is no better than none.
    if let Some(id) = element.attrs.get("id").filter(|id| fingerprint::is_stable_attr_id(id)) {
        return format!("id:{id}");
    }
    match element.attrs.get("type") {
        Some(kind) => format!("<{}[{kind}]>#{position}", element.tag),
        None => format!("<{}>#{position}", element.tag),
    }
}

/// Groups elements into components, in order of first appearance.
pub fn group(elements: Vec<Located>) -> Vec<Component> {
    let analysed = analyse(&elements);

    // A slot is templated when it appears in enough items and its names really vary.
    let mut slots: HashMap<&Slot, (u64, HashSet<String>)> = HashMap::new();
    for (located, info) in elements.iter().zip(&analysed) {
        if let Some(slot) = &info.slot {
            let (size, names) = slots.entry(slot).or_default();
            *size += 1;
            names.insert(fingerprint::normalize_name(&located.element.name));
        }
    }
    let templated: HashSet<Slot> = slots
        .into_iter()
        .filter(|(_, (size, names))| *size >= REPEAT_THRESHOLD && names.len() as u64 >= REPEAT_THRESHOLD)
        .map(|(slot, _)| slot.clone())
        .collect();

    let mut components: Vec<Component> = Vec::new();
    let mut index_by_id: HashMap<String, usize> = HashMap::new();
    // Unnamed elements seen so far at each place. Three icon buttons in a row are three controls,
    // while the one icon button in each card of a grid is the same control twelve times.
    let mut unnamed_seen: HashMap<(String, String, String, String), usize> = HashMap::new();
    // Where each instance was found, in the order of `components[..].instances`, for the locators.
    let mut places: Vec<Vec<Place>> = Vec::new();
    for (located, info) in elements.into_iter().zip(analysed) {
        let Located {
            element,
            frame_id,
            page_box,
            name_changes,
            frame_step,
        } = located;
        let template_ordinal = info
            .slot
            .as_ref()
            .filter(|slot| templated.contains(slot))
            .map(|slot| slot.2);
        let name_key = match template_ordinal {
            Some(0) => TEMPLATE.to_string(),
            // A second templated control in the same item is a different component.
            Some(ordinal) => format!("{TEMPLATE}#{ordinal}"),
            // A countdown or a running total: the text is data, the control is the same.
            None if name_changes => CHANGING.to_string(),
            None if element.name.is_empty() => {
                let item: String = element
                    .path
                    .iter()
                    .map(|step| format!("{:?}/", step.repeat_index))
                    .collect();
                let place = (frame_id.clone(), info.path_key.clone(), element.role.clone(), item);
                let position = unnamed_seen.entry(place).or_insert(0);
                *position += 1;
                unnamed_key(&element, *position - 1)
            }
            None => fingerprint::normalize_name(&element.name),
        };
        let id = fingerprint::component_id(&element.role, &name_key, &info.path_key);

        let index = *index_by_id.entry(id.clone()).or_insert_with(|| {
            components.push(Component {
                id: id.clone(),
                role: element.role.clone(),
                name: if template_ordinal.is_some() {
                    TEMPLATE.to_string()
                } else {
                    element.name.clone()
                },
                name_is_template: template_ordinal.is_some(),
                name_changes: name_changes.then_some(true),
                confidence: element.interactive.confidence,
                signals: Vec::new(),
                path: info.path_key.clone(),
                crop: None,
                locator: None,
                instances: Vec::new(),
            });
            components.len() - 1
        });
        let component = &mut components[index];
        if element.interactive.confidence == Confidence::Low {
            component.confidence = Confidence::Low;
        }
        for signal in element.interactive.signals {
            if !component.signals.contains(&signal) {
                component.signals.push(signal);
            }
        }
        if places.len() <= index {
            places.resize_with(index + 1, Vec::new);
        }
        places[index].push(Place {
            frame_step,
            path: element.path.clone(),
            text_name: element.name_source.as_deref() == Some("content"),
        });
        component.instances.push(Instance {
            key: format!("{id}:{}", component.instances.len()),
            name: element.name,
            name_source: element.name_source,
            tag: element.tag,
            attrs: element.attrs,
            selectors: element.selectors,
            box_: page_box.unwrap_or(element.box_),
            state: element.state,
            frame_id,
            in_shadow_dom: element.in_shadow_dom,
        });
    }
    locator::assign(&mut components, &places);
    components
}

#[cfg(test)]
mod tests {
    use super::*;
    use traceqa_common::types::{ElementState, InteractiveEvidence, PathStep};

    fn step(role: &str, tag: &str, repeat: Option<(u64, u64)>) -> PathStep {
        PathStep {
            role: role.into(),
            tag: tag.into(),
            name: None,
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

    /// A list of `count` items under `container`, each holding a link named by `name` and an "Add" button.
    fn cards(container: &str, count: u64, name: impl Fn(u64) -> String) -> Vec<Located> {
        (0..count)
            .flat_map(|i| {
                let path = vec![step(container, "ul", None), step("listitem", "li", Some((i, count)))];
                [element("link", &name(i), path.clone()), element("button", "Add", path)]
            })
            .collect()
    }

    fn summary(components: &[Component]) -> Vec<(String, String, usize)> {
        components
            .iter()
            .map(|c| (c.role.clone(), c.name.clone(), c.instances.len()))
            .collect()
    }

    #[test]
    fn each_instance_keeps_where_its_own_name_came_from() {
        let path = vec![step("banner", "header", None)];
        let mut by_label = element("textbox", "Email", path.clone());
        by_label.element.name_source = Some("label".into());
        let mut by_placeholder = element("textbox", "Email", path);
        by_placeholder.element.name_source = Some("placeholder".into());
        let components = group(vec![by_label, by_placeholder]);
        assert_eq!(components.len(), 1);
        let sources: Vec<Option<&str>> = components[0]
            .instances
            .iter()
            .map(|i| i.name_source.as_deref())
            .collect();
        assert_eq!(sources, [Some("label"), Some("placeholder")]);
    }

    #[test]
    fn analyse_survives_an_unplaced_element() {
        // Four cards, and one element that sits in no repeated group, in the middle of them.
        let mut elements = cards("list", 4, |i| format!("Item {i}"));
        elements.insert(3, element("link", "Home", vec![step("banner", "header", None)]));
        let analysed = analyse(&elements);
        assert_eq!(analysed.len(), 9);
        assert!(analysed[3].slot.is_none());
        let carded: Vec<bool> = analysed.iter().map(|a| a.slot.is_some()).collect();
        assert_eq!(carded, [true, true, true, false, true, true, true, true, true]);
    }

    #[test]
    fn the_rows_of_a_table_are_its_items_not_its_cells() {
        // Six rows of four cells: a checkbox named after the row in the first cell, Edit and
        // Delete in the last. The cells repeat too, but the row is what is repeated.
        let at = |row: u64, cell: u64| {
            vec![
                step("table", "table", None),
                step("row", "tr", Some((row, 6))),
                step("cell", "td", Some((cell, 4))),
            ]
        };
        let mut elements = Vec::new();
        for row in 0..6 {
            elements.push(element("checkbox", &format!("Select person {row}"), at(row, 0)));
            elements.push(element("button", "Edit", at(row, 3)));
            elements.push(element("button", "Delete", at(row, 3)));
        }
        let components = group(elements);
        let shape: Vec<(&str, bool, usize)> = components
            .iter()
            .map(|c| (c.name.as_str(), c.name_is_template, c.instances.len()))
            .collect();
        assert_eq!(shape, [("*", true, 6), ("Edit", false, 6), ("Delete", false, 6)]);
    }

    #[test]
    fn a_name_seen_to_change_is_left_out_of_the_id() {
        let ticking = |name: &str| Located {
            name_changes: true,
            ..element("button", name, vec![])
        };
        let first = group(vec![ticking("Sale ends in 59 s"), element("button", "Buy", vec![])]);
        let later = group(vec![ticking("Sale ends in 12 s"), element("button", "Buy", vec![])]);
        // The countdown keeps its id while its text runs down, and shows the text it had last.
        assert_eq!(first[0].id, later[0].id);
        assert_eq!(
            (later[0].name.as_str(), later[0].name_changes),
            ("Sale ends in 12 s", Some(true))
        );
        // A name that stays put is part of the id as always.
        assert_eq!(first[1].id, later[1].id);
        assert_eq!(first[1].name_changes, None);
        assert_ne!(first[0].id, first[1].id);
    }

    #[test]
    fn a_grid_becomes_one_component_per_control() {
        let components = group(cards("list", 12, |i| format!("Product {i}")));
        assert_eq!(
            summary(&components),
            [("link".into(), "*".into(), 12), ("button".into(), "Add".into(), 12)]
        );
        assert!(components[0].name_is_template && !components[1].name_is_template);
        assert_eq!(components[0].instances[3].key, format!("{}:3", components[0].id));
        assert_eq!(components[0].instances[3].name, "Product 3");
    }

    #[test]
    fn navigation_items_stay_separate() {
        let components = group(cards("navigation", 4, |i| format!("Page {i}")));
        let links = components.iter().filter(|c| c.role == "link").count();
        assert_eq!(links, 4);
        assert!(components.iter().all(|c| !c.name_is_template));
    }

    #[test]
    fn a_list_of_single_links_is_not_a_grid_of_cards() {
        // A footer: six items, one link each. They repeat, but each is its own control.
        let footer: Vec<Located> = (0..6)
            .map(|i| {
                let path = vec![step("list", "ul", None), step("listitem", "li", Some((i, 6)))];
                element(
                    "link",
                    ["About", "Contact", "Founders", "Privacy", "Terms", "Careers"][i as usize],
                    path,
                )
            })
            .collect();
        let components = group(footer);
        assert_eq!(components.len(), 6);
        assert!(components.iter().all(|c| !c.name_is_template && c.instances.len() == 1));
    }

    #[test]
    fn sections_that_merely_repeat_are_not_cards() {
        // Three page sections, each a repeated <div> holding controls, but arranged differently.
        let section = |i| vec![step("main", "main", None), step("generic", "div", Some((i, 3)))];
        let page = vec![
            element("link", "Explore products", section(0)),
            element("link", "Start a project", section(0)),
            element("link", "Meet the founders", section(1)),
            element("button", "Play video", section(1)),
            element("link", "Write to us", section(2)),
            element("link", "Start a project", section(2)),
            element("link", "Get early access", section(2)),
        ];
        let components = group(page);
        assert!(components.iter().all(|c| !c.name_is_template));
        // The same link in two sections is still one component with two instances.
        let start = components.iter().find(|c| c.name == "Start a project").unwrap();
        assert_eq!(start.instances.len(), 2);
        assert_eq!(components.len(), 6);
    }

    #[test]
    fn two_names_are_not_enough_to_template() {
        // Eleven "In stock" and one "Sold out" are two components, not a template.
        let components = group(cards("list", 12, |i| {
            if i == 6 {
                "Sold out".into()
            } else {
                "In stock".into()
            }
        }));
        assert_eq!(
            summary(&components),
            [
                ("link".into(), "In stock".into(), 11),
                ("button".into(), "Add".into(), 12),
                ("link".into(), "Sold out".into(), 1)
            ]
        );
    }

    #[test]
    fn ids_do_not_depend_on_how_many_items_loaded() {
        let few = group(cards("list", 3, |i| format!("Product {i}")));
        let many = group(cards("list", 9, |i| format!("Product {i}")));
        let ids = |components: &[Component]| components.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&few), ids(&many));
    }

    #[test]
    fn unnamed_elements_are_told_apart_by_their_attributes() {
        let mut cart = element("button", "", vec![]);
        cart.element.attrs.insert("id".into(), "cart-btn".into());
        let mut menu = element("button", "", vec![]);
        menu.element.attrs.insert("id".into(), "menu-btn".into());
        let components = group(vec![cart, menu]);
        assert_eq!(components.len(), 2);
        assert_ne!(components[0].id, components[1].id);
    }

    fn unnamed_with_id(id: &str) -> Located {
        let mut located = element("button", "", vec![]);
        located.element.attrs.insert("id".into(), id.into());
        located
    }

    #[test]
    fn an_unnamed_control_with_a_generated_id_is_keyed_by_position() {
        // The same page loaded twice: the framework hands out ':r1:' then ':r7:'.
        let first = group(vec![unnamed_with_id(":r1:")]);
        let second = group(vec![unnamed_with_id(":r7:")]);
        assert_eq!(first[0].id, second[0].id);

        // A readable id still tells the control apart from a sibling without one.
        let toggle = group(vec![unnamed_with_id("search-toggle"), element("button", "", vec![])]);
        assert_eq!(toggle.len(), 2);
        assert_ne!(toggle[0].id, toggle[1].id);
        let again = group(vec![unnamed_with_id("search-toggle")]);
        assert_eq!(toggle[0].id, again[0].id);
    }

    #[test]
    fn unstable_id_falls_through_to_tag_type_position() {
        let mut typed = unnamed_with_id("_x");
        typed.element.attrs.insert("type".into(), "submit".into());
        let bare = element("button", "", vec![]);
        let mut typed_without_id = element("button", "", vec![]);
        typed_without_id.element.attrs.insert("type".into(), "submit".into());
        // An unstable id is as good as no id: the keys are the tag, type and position.
        assert_eq!(
            unnamed_key(&typed.element, 0),
            unnamed_key(&typed_without_id.element, 0)
        );
        assert_eq!(unnamed_key(&typed.element, 0), "<button[submit]>#0");
        assert_eq!(unnamed_key(&unnamed_with_id(":r1:").element, 2), "<button>#2");
        assert_eq!(unnamed_key(&bare.element, 1), "<button>#1");
    }

    #[test]
    fn unnamed_elements_side_by_side_are_different_controls() {
        // Previous, play and next: three icon buttons with no name and nothing else to tell them apart.
        let player = vec![
            element("button", "", vec![]),
            element("button", "", vec![]),
            element("button", "", vec![]),
        ];
        assert_eq!(group(player).len(), 3);

        // One icon button in each of four cards is the same control four times.
        let cards: Vec<Located> = (0..4)
            .map(|i| {
                element(
                    "button",
                    "",
                    vec![step("list", "ul", None), step("listitem", "li", Some((i, 4)))],
                )
            })
            .collect();
        let components = group(cards);
        assert_eq!(summary(&components), [("button".into(), "".into(), 4)]);
    }

    fn named(role: &str, tag: &str, name: &str) -> PathStep {
        PathStep {
            name: Some(name.into()),
            ..step(role, tag, None)
        }
    }

    /// A contents list in a sidebar, as a wide window has it (`wrapper` is a plain `div`) or as a
    /// narrow one has it (the same navigation moved into `<main>`), followed by a grid of cards.
    fn page_with_contents(wrapper: PathStep) -> Vec<Located> {
        let toc = vec![
            step("generic", "div", None),
            wrapper,
            named("navigation", "nav", "Contents"),
        ];
        let mut elements = vec![
            element("link", "History", toc.clone()),
            element("link", "Geography", toc.clone()),
            element("button", "", toc),
        ];
        let grid = |i| {
            vec![
                step("generic", "div", None),
                step("main", "main", None),
                step("list", "ul", None),
                step("listitem", "li", Some((i, 4))),
            ]
        };
        for i in 0..4 {
            elements.push(element("link", &format!("Product {i}"), grid(i)));
            elements.push(element("button", "Add", grid(i)));
        }
        elements
    }

    #[test]
    fn reparenting_a_named_landmark_keeps_ids() {
        let wide = group(page_with_contents(step("generic", "div", None)));
        let narrow = group(page_with_contents(step("main", "main", None)));
        let ids = |components: &[Component]| components.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&wide), ids(&narrow));
        let history = wide.iter().find(|c| c.name == "History").unwrap();
        assert_eq!(history.path, "navigation \"contents\"");
        // The unnamed icon button in the contents list is keyed the same way in both.
        assert!(wide
            .iter()
            .any(|c| c.name.is_empty() && c.path == "navigation \"contents\""));
    }

    #[test]
    fn cards_still_template_after_the_change() {
        let components = group(page_with_contents(step("generic", "div", None)));
        let product = components.iter().find(|c| c.name_is_template).unwrap();
        assert_eq!(product.instances.len(), 4);
        // The cards have no named ancestor, so their paths are whole.
        assert_eq!(product.path, "div > main > list > listitem");
        assert_eq!(components.iter().filter(|c| c.name == "Add").count(), 1);
    }

    #[test]
    fn the_same_control_in_another_frame_is_another_component() {
        let mut framed = element("button", "Subscribe", vec![]);
        framed.frame_id = "f1".into();
        framed.frame_step = Some("frame \"newsletter\"".into());
        framed.page_box = Some(Rect {
            x: 100.0,
            y: 200.0,
            w: 10.0,
            h: 10.0,
        });
        let components = group(vec![element("button", "Subscribe", vec![]), framed]);
        assert_eq!(components.len(), 2);
        assert_eq!(components[1].path, "frame \"newsletter\"");
        assert_eq!(components[1].instances[0].box_.x, 100.0);
    }

    #[test]
    fn the_first_templated_control_of_an_item_is_star_and_the_next_is_numbered_from_one() {
        // Two links per card, both named by the product: same role, same place, told apart by order.
        let elements: Vec<Located> = (0..4)
            .flat_map(|i| {
                let path = vec![step("list", "ul", None), step("listitem", "li", Some((i, 4)))];
                [
                    element("link", &format!("Title {i}"), path.clone()),
                    element("link", &format!("Image of {i}"), path),
                ]
            })
            .collect();
        let components = group(elements);
        assert_eq!(components.len(), 2);
        let path = &components[0].path;
        assert_eq!(components[0].id, fingerprint::component_id("link", "*", path));
        assert_eq!(components[1].id, fingerprint::component_id("link", "*#1", path));
        assert_eq!(components[0].instances[2].name, "Title 2");
        assert_eq!(components[1].instances[2].name, "Image of 2");
    }

    #[test]
    fn unnamed_siblings_are_keyed_by_their_position_counted_from_zero() {
        let components = group(vec![
            element("button", "", vec![]),
            element("button", "", vec![]),
            element("button", "", vec![]),
        ]);
        let ids: Vec<&str> = components.iter().map(|c| c.id.as_str()).collect();
        let expected: Vec<String> = (0..3)
            .map(|n| fingerprint::component_id("button", &format!("<button>#{n}"), ""))
            .collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn one_low_confidence_instance_makes_the_component_low_and_signals_are_listed_once() {
        let mut weak = element("button", "Go", vec![]);
        weak.element.interactive = InteractiveEvidence {
            confidence: Confidence::Low,
            signals: vec!["native".into(), "cursor".into()],
        };
        let strong = element("button", "Go", vec![]);
        let components = group(vec![strong, weak, element("button", "Go", vec![])]);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].instances.len(), 3);
        // The first instance was High; the later Low one wins, and a later High one does not undo it.
        assert_eq!(components[0].confidence, Confidence::Low);
        // "native" came from all three instances and is kept once; "cursor" was new.
        assert_eq!(components[0].signals, ["native", "cursor"]);
        // Instances that are all High leave the component High.
        let calm = group(vec![element("button", "Go", vec![]), element("button", "Go", vec![])]);
        assert_eq!(calm[0].confidence, Confidence::High);
    }
}
