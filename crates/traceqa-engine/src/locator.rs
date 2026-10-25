//! Works out how to find each component again with a Playwright-style locator.
//!
//! A [`Locator`] is a description, not code: `traceqa-protocol` renders it as TypeScript or
//! Python. It is computed here, at grouping time, because only here are the structured path
//! steps (role and name of each ancestor) still at hand.
//!
//! Uniqueness is decided the way Playwright decides what a locator matches: with `exact` a
//! name must equal the accessible name (whitespace normalised, case sensitive), and hidden
//! elements are not found unless `includeHidden` is on. A locator is *unique* when it matches
//! every instance of its component and nothing else that was collected. The counts only cover
//! the elements of this one page in this one browser, so `traceqa codegen` combines the files
//! of all browsers before it calls a component unique.
//!
//! The order of preference: role and exact name; the shared `data-testid`; role and name
//! inside the nearest ancestors that tell it apart; the exact text; and, last, a structural
//! CSS selector marked `forced`. `nth()` is never used: it picks an element silently.

use traceqa_protocol::types::{Component, Locator, LocatorScope, LocatorStrategy, PathStep};

use crate::fingerprint::REPEAT_THRESHOLD;

/// Where one instance sits: its frame and the ancestors above it.
pub struct Place {
    /// The frame step of the path, for example `frame "pay"`. `None` in the main frame.
    pub frame_step: Option<String>,
    pub path: Vec<PathStep>,
    /// The name is the element's own visible text, which `getByText` can find.
    pub text_name: bool,
}

/// Whether Playwright gives `role` to an element with this tag, as the collector did. The
/// collector reports a `summary` as a button and a `contenteditable` element as a textbox; the
/// browsers' accessibility trees, which `getByRole` reads, do not.
fn playwright_agrees(tag: &str, role: &str) -> bool {
    tag != "summary" && !(role == "textbox" && !matches!(tag, "input" | "textarea"))
}

/// Roles a browser exposes and `getByRole` accepts (WAI-ARIA 1.2 concrete roles). `generic`,
/// `none` and `presentation` are left out: they have no name to look for.
const ARIA_ROLES: &[&str] = &[
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "grid",
    "gridcell",
    "group",
    "heading",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "marquee",
    "math",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "meter",
    "navigation",
    "note",
    "option",
    "paragraph",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
];

/// Ancestors without a name that still say where something is.
const STRUCTURE: &[&str] = &[
    "banner",
    "main",
    "navigation",
    "contentinfo",
    "complementary",
    "region",
    "form",
    "search",
    "dialog",
    "alertdialog",
    "list",
    "table",
    "grid",
    "group",
    "tablist",
    "menu",
    "toolbar",
];

/// Most ids listed in `ambiguousWith`.
const MAX_AMBIGUOUS: usize = 5;

fn is_aria_role(role: &str) -> bool {
    ARIA_ROLES.contains(&role)
}

/// Whitespace collapsed and trimmed, the way Playwright compares an exact name.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The label inside `frame "label"`, with the escapes of a path key undone.
fn frame_label(step: &str) -> String {
    let inner = step
        .strip_prefix("frame \"")
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(step);
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// One collected element, with what a locator can look at.
struct Seen<'a> {
    component: usize,
    role: &'a str,
    name: String,
    visible: bool,
    test_id: Option<&'a str>,
    frame: Option<String>,
    path: &'a [PathStep],
}

/// What a locator asks for.
#[derive(Clone)]
struct Query {
    strategy: LocatorStrategy,
    role: Option<String>,
    name: Option<String>,
    test_id: Option<String>,
    scope: Vec<LocatorScope>,
    frame: Option<String>,
    include_hidden: bool,
}

impl Query {
    fn matches(&self, seen: &Seen) -> bool {
        if seen.frame != self.frame || !(seen.visible || self.include_hidden) {
            return false;
        }
        let own = match self.strategy {
            LocatorStrategy::Role => {
                self.role.as_deref() == Some(seen.role) && self.name.as_ref().is_none_or(|name| *name == seen.name)
            }
            LocatorStrategy::Testid => self.test_id.as_deref() == seen.test_id && seen.test_id.is_some(),
            LocatorStrategy::Text => self.name.as_ref() == Some(&seen.name),
            LocatorStrategy::Css => false,
        };
        own && self.in_scope(seen.path)
    }

    /// The scope entries appear among the ancestors in order, each below the one before.
    fn in_scope(&self, path: &[PathStep]) -> bool {
        let mut wanted = self.scope.iter().peekable();
        for step in path {
            if let Some(scope) = wanted.peek() {
                let same_name = scope
                    .name
                    .as_ref()
                    .is_none_or(|name| step.name.as_deref().map(squash).as_ref() == Some(name));
                if step.role == scope.role && same_name {
                    wanted.next();
                }
            }
        }
        wanted.peek().is_none()
    }
}

/// How well a query does for one component.
struct Verdict {
    matches: usize,
    own: usize,
    /// Every instance the component shows is matched, and nothing else is.
    unique: bool,
    ambiguous_with: Vec<usize>,
}

fn judge(query: &Query, component: usize, everything: &[Seen]) -> Verdict {
    let mut matches = 0;
    let mut own = 0;
    let mut also: Vec<usize> = Vec::new();
    for seen in everything.iter().filter(|seen| query.matches(seen)) {
        matches += 1;
        if seen.component == component {
            own += 1;
        } else if !also.contains(&seen.component) {
            also.push(seen.component);
        }
    }
    let expected = everything
        .iter()
        .filter(|seen| seen.component == component && (seen.visible || query.include_hidden))
        .count();
    Verdict {
        matches,
        own,
        unique: matches == own && own == expected && own > 0,
        ambiguous_with: also,
    }
}

/// Roles whose names the page's author wrote, even when several sit side by side.
const NAMED_LANDMARKS: &[&str] = &[
    "banner",
    "main",
    "navigation",
    "contentinfo",
    "complementary",
    "region",
    "form",
    "search",
];

/// One card among many: its name is data and differs from card to card.
fn is_data(step: &PathStep) -> bool {
    step.repeat_count.unwrap_or(0) >= REPEAT_THRESHOLD && !NAMED_LANDMARKS.contains(&step.role.as_str())
}

/// The ancestors worth searching inside, nearest first: a real role, a name or a structural
/// role, and not one card among many (its name is data).
fn scope_candidates(path: &[PathStep]) -> Vec<LocatorScope> {
    path.iter()
        .rev()
        .filter(|step| is_aria_role(&step.role) && playwright_agrees(&step.tag, &step.role) && !is_data(step))
        .filter_map(|step| {
            let name = step.name.as_deref().map(squash).filter(|name| !name.is_empty());
            (name.is_some() || STRUCTURE.contains(&step.role.as_str())).then(|| LocatorScope {
                role: step.role.clone(),
                name,
            })
        })
        .collect()
}

fn into_locator(query: Query, verdict: &Verdict, ids: &[String]) -> Locator {
    Locator {
        strategy: query.strategy,
        exact: query.name.is_some().then_some(true),
        role: query.role,
        name: query.name,
        test_id: query.test_id,
        scope: query.scope,
        frame: query.frame,
        css: None,
        unique: verdict.unique,
        matches: verdict.matches as u64,
        own: verdict.own as u64,
        ambiguous_with: verdict
            .ambiguous_with
            .iter()
            .take(MAX_AMBIGUOUS)
            .map(|&index| ids[index].clone())
            .collect(),
        forced: false,
        needs_include_hidden: query.include_hidden,
    }
}

/// Adds a locator to every component. `places[c][i]` is where instance `i` of component `c`
/// was found.
pub fn assign(components: &mut [Component], places: &[Vec<Place>]) {
    let ids: Vec<String> = components.iter().map(|c| c.id.clone()).collect();
    let everything: Vec<Seen> = components
        .iter()
        .zip(places)
        .enumerate()
        .flat_map(|(index, (component, places))| {
            component
                .instances
                .iter()
                .zip(places)
                .map(move |(instance, place)| Seen {
                    component: index,
                    // A role Playwright would not see never matches a getByRole.
                    role: if playwright_agrees(&instance.tag, &component.role) {
                        &component.role
                    } else {
                        ""
                    },
                    name: squash(&instance.name),
                    visible: instance.state.visible,
                    test_id: instance.attrs.get("data-testid").map(String::as_str),
                    frame: place.frame_step.as_deref().map(frame_label),
                    path: &place.path,
                })
        })
        .collect();

    let locators: Vec<Locator> = components
        .iter()
        .zip(places)
        .enumerate()
        .map(|(index, (component, places))| locate(index, component, places, &everything, &ids))
        .collect();
    for (component, locator) in components.iter_mut().zip(locators) {
        component.locator = Some(locator);
    }
}

/// The `data-testid` every instance carries, when they all carry the same.
fn shared_test_id(component: &Component) -> Option<String> {
    let mut ids = component.instances.iter().map(|i| i.attrs.get("data-testid"));
    let first = ids.next()??.clone();
    (!first.is_empty() && ids.all(|id| id == Some(&first))).then_some(first)
}

fn locate(index: usize, component: &Component, places: &[Place], everything: &[Seen], ids: &[String]) -> Locator {
    let frame = places
        .first()
        .and_then(|place| place.frame_step.as_deref())
        .map(frame_label);
    let path: &[PathStep] = places.first().map_or(&[], |place| &place.path);
    let include_hidden = !component.instances.iter().any(|i| i.state.visible);
    let role_ok = is_aria_role(&component.role)
        && component
            .instances
            .iter()
            .all(|instance| playwright_agrees(&instance.tag, &component.role));
    let name = squash(&component.name);
    let named = !name.is_empty() && !component.name_is_template && component.name_changes != Some(true);
    let query = |strategy, role: Option<&str>, name: Option<&str>, test_id: Option<String>| Query {
        strategy,
        role: role.map(str::to_string),
        name: name.map(str::to_string),
        test_id,
        scope: Vec::new(),
        frame: frame.clone(),
        include_hidden,
    };

    // Each base query, and whether ancestors may be added to it.
    let mut bases: Vec<(Query, bool)> = Vec::new();
    if role_ok && named {
        bases.push((
            query(LocatorStrategy::Role, Some(&component.role), Some(&name), None),
            false,
        ));
    }
    if let Some(test_id) = shared_test_id(component) {
        bases.push((query(LocatorStrategy::Testid, None, None, Some(test_id)), false));
    }
    if role_ok {
        let name = named.then_some(name.as_str());
        bases.push((query(LocatorStrategy::Role, Some(&component.role), name, None), true));
    } else if named && places.iter().all(|place| place.text_name) {
        bases.push((query(LocatorStrategy::Text, None, Some(&name), None), true));
    }

    let mut tried: Vec<(Query, Verdict)> = Vec::new();
    for (base, may_scope) in bases {
        let verdict = judge(&base, index, everything);
        if verdict.unique {
            return into_locator(base, &verdict, ids);
        }
        let mut last = (base.clone(), verdict);
        if may_scope {
            let nearest_first = scope_candidates(path);
            for count in 1..=nearest_first.len() {
                let mut scoped = base.clone();
                scoped.scope = nearest_first[..count].iter().rev().cloned().collect();
                let verdict = judge(&scoped, index, everything);
                if verdict.unique {
                    return into_locator(scoped, &verdict, ids);
                }
                last = (scoped, verdict);
            }
        }
        tried.push(last);
    }

    // Nothing readable tells the component apart. A structural selector finds exactly its
    // instances, whatever their state, but breaks when the page is rearranged.
    let selectors: Option<Vec<&str>> = component
        .instances
        .iter()
        .map(|instance| instance.selectors.first().map(String::as_str))
        .collect();
    if let Some(selectors) = selectors.filter(|selectors| !selectors.is_empty()) {
        // Playwright's CSS engine goes into open shadow roots by itself, so ` >>> ` becomes a space.
        let css = selectors
            .iter()
            .map(|selector| selector.replace(" >>> ", " "))
            .collect::<Vec<_>>()
            .join(", ");
        let total = component.instances.len() as u64;
        return Locator {
            strategy: LocatorStrategy::Css,
            role: None,
            name: None,
            exact: None,
            test_id: None,
            scope: Vec::new(),
            frame,
            css: Some(css),
            unique: true,
            matches: total,
            own: total,
            ambiguous_with: Vec::new(),
            forced: true,
            needs_include_hidden: false,
        };
    }

    // No selector either: report the closest readable locator, honestly not unique.
    let closest = tried.into_iter().min_by_key(|(_, verdict)| {
        (
            verdict.matches - verdict.own.min(verdict.matches),
            usize::MAX - verdict.own,
        )
    });
    match closest {
        Some((query, verdict)) => into_locator(query, &verdict, ids),
        None => Locator {
            strategy: LocatorStrategy::Css,
            role: None,
            name: None,
            exact: None,
            test_id: None,
            scope: Vec::new(),
            frame,
            css: Some(component.instances.first().map_or("*", |i| i.tag.as_str()).to_string()),
            unique: false,
            matches: 0,
            own: 0,
            ambiguous_with: Vec::new(),
            forced: false,
            needs_include_hidden: false,
        },
    }
}

#[cfg(test)]
#[path = "locator_tests.rs"]
mod tests;
