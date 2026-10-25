//! The page outline behind `describe_page`, and the rows shared by the tools that list components.
//!
//! A component's `path` is a readable string such as `main > list "products" > listitem`: steps
//! joined by ` > `, each step a role, or a role and a quoted name. A name may itself contain
//! ` > ` or a quote (escaped with a backslash), so the path is split with a reader that knows
//! about quotes and not with a plain `split`.

use serde_json::{json, Map, Value};
use traceqa_protocol::types::{Component, ComponentsFile};

/// Roles that start a region of the page. The first of these on a path names its group.
const LANDMARKS: [&str; 9] = [
    "banner",
    "navigation",
    "main",
    "region",
    "complementary",
    "contentinfo",
    "form",
    "search",
    "dialog",
];

/// The name of the group for a path that has no steps.
const NO_PATH: &str = "(page)";
/// The most example names shown for a component whose instances are named differently.
const EXAMPLE_NAMES: usize = 3;

/// Splits a path into its steps. A separator inside quotes, or after a backslash, is text.
pub(crate) fn split_path(path: &str) -> Vec<&str> {
    let bytes = path.as_bytes();
    let mut steps = Vec::new();
    let (mut start, mut at) = (0, 0);
    let (mut quoted, mut escaped) = (false, false);
    while at < bytes.len() {
        let byte = bytes[at];
        if quoted {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => quoted = false,
                _ => {}
            }
        } else if byte == b'"' {
            quoted = true;
        } else if bytes[at..].starts_with(b" > ") {
            steps.push(&path[start..at]);
            at += 3;
            start = at;
            continue;
        }
        at += 1;
    }
    steps.push(&path[start..]);
    steps.retain(|step| !step.is_empty());
    steps
}

/// The role of a step: what comes before the name.
fn step_role(step: &str) -> &str {
    step.split(' ').next().unwrap_or_default()
}

/// The group a path belongs to: its steps up to and including the first landmark, else its
/// first step, else `(page)`.
pub(crate) fn group_key(path: &str) -> String {
    let steps = split_path(path);
    match steps.iter().position(|step| LANDMARKS.contains(&step_role(step))) {
        Some(landmark) => steps[..=landmark].join(" > "),
        None => steps.first().map_or_else(|| NO_PATH.to_string(), ToString::to_string),
    }
}

/// True when no instance of the component can be seen.
fn hidden(component: &Component) -> bool {
    !component.instances.is_empty() && component.instances.iter().all(|i| !i.state.visible)
}

/// The short row: `id`, `role`, `name` and `instances`, plus `hidden` or `template` when true.
pub(crate) fn concise_row(component: &Component) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert("id".into(), json!(component.id));
    row.insert("role".into(), json!(component.role));
    row.insert("name".into(), json!(component.name));
    row.insert("instances".into(), json!(component.instances.len()));
    if hidden(component) {
        row.insert("hidden".into(), json!(true));
    }
    if component.name_is_template {
        row.insert("template".into(), json!(true));
    }
    row
}

/// What `detailed` adds to a row.
pub(crate) fn add_detail(row: &mut Map<String, Value>, component: &Component) {
    let first = component.instances.first();
    row.insert(
        "visible".into(),
        json!(component.instances.iter().filter(|i| i.state.visible).count()),
    );
    row.insert("path".into(), json!(component.path));
    row.insert("selector".into(), json!(first.and_then(|i| i.selectors.first())));
    row.insert("confidence".into(), json!(component.confidence));
    row.insert("has_crop".into(), json!(component.crop.is_some()));
}

/// The outline of one page: components grouped by landmark, in page order.
pub(crate) fn describe(target: &str, file: &ComponentsFile, detailed: bool, max_per_group: usize) -> Value {
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<&Component>> = Vec::new();
    for component in &file.components {
        let key = group_key(&component.path);
        let at = order.iter().position(|known| *known == key).unwrap_or_else(|| {
            order.push(key);
            groups.push(Vec::new());
            order.len() - 1
        });
        groups[at].push(component);
    }
    let groups: Vec<Value> = order
        .iter()
        .zip(&groups)
        .map(|(key, members)| {
            let items: Vec<Value> = members
                .iter()
                .take(max_per_group)
                .map(|component| {
                    let mut row = concise_row(component);
                    if component.name_is_template {
                        row.insert("example_names".into(), json!(example_names(component)));
                    }
                    if detailed {
                        add_detail(&mut row, component);
                    }
                    Value::Object(row)
                })
                .collect();
            let mut group = json!({ "group": key, "count": members.len(), "items": items });
            if members.len() > max_per_group {
                group["more"] = json!(members.len() - max_per_group);
            }
            group
        })
        .collect();
    let mut outline = json!({
        "target": target,
        "title": file.title,
        "total": file.components.len(),
        "groups": groups,
    });
    if detailed {
        outline["url"] = json!(file.url);
        outline["browser"] = json!(file.browser);
        outline["viewport"] = json!(file.viewport);
        outline["warnings"] = json!(file.warnings);
    } else if !file.warnings.is_empty() {
        outline["warnings"] = json!(file.warnings);
    }
    outline
}

/// A few different names among a component's instances.
fn example_names(component: &Component) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for instance in &component.instances {
        if names.len() == EXAMPLE_NAMES {
            break;
        }
        if !names.contains(&instance.name.as_str()) {
            names.push(&instance.name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_the_separator_between_steps() {
        assert_eq!(
            split_path("main > list \"products\" > listitem"),
            ["main", "list \"products\"", "listitem"]
        );
        assert_eq!(split_path("button"), ["button"]);
        assert!(split_path("").is_empty());
    }

    #[test]
    fn a_name_may_hold_the_separator() {
        assert_eq!(split_path("region \"a > b\" > button"), ["region \"a > b\"", "button"]);
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_name() {
        // region "say \"x > y\" now" > link
        let path = r#"region "say \"x > y\" now" > link"#;
        assert_eq!(split_path(path), [r#"region "say \"x > y\" now""#, "link"]);
    }

    #[test]
    fn a_name_ending_in_an_escaped_backslash_ends_at_the_next_quote() {
        // The name is `a\` (escaped as `a\\`), so the quote after it is the closing one.
        let path = r#"region "a\\" > button"#;
        assert_eq!(split_path(path), [r#"region "a\\""#, "button"]);
    }

    #[test]
    fn a_frame_step_is_a_step_like_any_other() {
        assert_eq!(
            split_path(r#"frame "pay > now" > main > form"#),
            [r#"frame "pay > now""#, "main", "form"]
        );
    }

    #[test]
    fn the_group_is_the_path_to_the_first_landmark() {
        assert_eq!(group_key("banner > navigation \"Main\""), "banner");
        assert_eq!(group_key("list \"x\" > main > button"), "list \"x\" > main");
        assert_eq!(group_key("list \"x\" > listitem"), "list \"x\"");
        assert_eq!(group_key(""), "(page)");
        assert_eq!(group_key("frame \"pay\" > main > form"), "frame \"pay\" > main");
        assert_eq!(group_key("frame \"pay\" > list"), "frame \"pay\"");
        assert_eq!(group_key(r#"region "a > b \" c" > button"#), r#"region "a > b \" c""#);
    }

    #[test]
    fn a_role_that_only_starts_like_a_landmark_is_not_one() {
        assert_eq!(group_key("mainframe > button"), "mainframe");
        assert_eq!(group_key("dialog \"Sign in\" > button"), "dialog \"Sign in\"");
    }
}
