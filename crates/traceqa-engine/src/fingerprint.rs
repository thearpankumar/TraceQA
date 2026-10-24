//! The component id. The recipe is specified in `spec/fingerprint.md`; change them together.

use sha2::{Digest, Sha256};
use traceqa_protocol::types::PathStep;
use unicode_normalization::UnicodeNormalization;

pub const VERSION: u64 = 2;

/// A repeated ancestor with this many siblings is treated as data, not structure.
pub const REPEAT_THRESHOLD: u64 = 3;

/// Name key of a component whose instances have different names.
pub const TEMPLATE: &str = "*";

/// Name key of a component whose name was seen to change while the page was read.
pub const CHANGING: &str = "~";

/// `c_` plus the first 12 hex characters of the hash.
pub fn component_id(role: &str, name_key: &str, path_key: &str) -> String {
    let digest = Sha256::digest(format!("v{VERSION}\n{role}\n{name_key}\n{path_key}"));
    let hex: String = digest.iter().take(6).map(|byte| format!("{byte:02x}")).collect();
    format!("c_{hex}")
}

/// Unicode NFC, trimmed, inner whitespace collapsed, lower-cased.
pub fn normalize_name(name: &str) -> String {
    let composed: String = name.nfc().collect();
    composed.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Whether an `id` attribute is one the page's author chose, as opposed to one a framework
/// generates. A port of `stableId` in `collector/collector.js`; `spec/fingerprint-vectors.json`
/// keeps the two in step. Generated ids start with a digit, `:` or `_`, or hold four digits in a
/// row or eight lower-case hex digits in a row (upper case does not count, as in the collector).
pub fn is_stable_attr_id(id: &str) -> bool {
    // JavaScript's `trim`, which the collector applies to every attribute it reads.
    let id = id.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let Some(first) = id.chars().next() else {
        return false;
    };
    if first.is_ascii_digit() || first == ':' || first == '_' {
        return false;
    }
    let (mut digits, mut hex) = (0, 0);
    for c in id.chars() {
        digits = if c.is_ascii_digit() { digits + 1 } else { 0 };
        hex = if c.is_ascii_digit() || ('a'..='f').contains(&c) {
            hex + 1
        } else {
            0
        };
        if digits >= 4 || hex >= 8 {
            return false;
        }
    }
    true
}

/// Roles that name a part of the page. Their names are written by the page's author, so they
/// are structure even when several of them sit side by side.
const LANDMARKS: [&str; 8] = [
    "banner",
    "main",
    "navigation",
    "contentinfo",
    "complementary",
    "region",
    "form",
    "search",
];

/// A step name as it appears between the quotes of a path key: normalised like a name, then
/// with `\` and `"` escaped, so a name can never end its quotes early and a name holding
/// ` > ` can never be read as two steps.
fn escape_step_name(name: &str) -> String {
    normalize_name(name).replace('\\', "\\\\").replace('"', "\\\"")
}

/// The first step of the path of an element inside a frame, from the frame's name or URL path.
pub fn frame_step_key(label: &str) -> String {
    format!("frame \"{}\"", escape_step_name(label))
}

/// One step of a path key: the role (the tag when there is no role), plus the name in quotes.
/// A repeated step that is not a landmark loses its name, because the name of one card among
/// many is data. Indexes are always left out, so every card in a grid has the same path.
pub fn step_key(step: &PathStep) -> String {
    let base = if step.role == "generic" { &step.tag } else { &step.role };
    match step_name(step) {
        Some(name) => format!("{base} \"{}\"", escape_step_name(name)),
        None => base.clone(),
    }
}

/// The name a step contributes to the path key: none when it has no name or the name is data.
fn step_name(step: &PathStep) -> Option<&str> {
    let is_data = step.repeat_count.unwrap_or(0) >= REPEAT_THRESHOLD && !LANDMARKS.contains(&step.role.as_str());
    step.name.as_deref().filter(|_| !is_data)
}

/// The path key of an element: the frame step (always kept), then the steps from the innermost
/// one the page gave a name down to the element. What sits above that step is plain wrappers and
/// unnamed landmarks, which a responsive layout moves around (a contents list that moves from a
/// sidebar `div` into `<main>` on a phone); the named step is the page's own stable vocabulary.
/// With no named step, the whole path is kept.
pub fn path_key(frame_step: Option<&str>, steps: &[PathStep]) -> String {
    let start = steps.iter().rposition(|step| step_name(step).is_some()).unwrap_or(0);
    frame_step
        .map(str::to_string)
        .into_iter()
        .chain(steps[start..].iter().map(step_key))
        .collect::<Vec<_>>()
        .join(" > ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(role: &str, tag: &str, name: Option<&str>, repeat: Option<(u64, u64)>) -> PathStep {
        PathStep {
            role: role.into(),
            tag: tag.into(),
            name: name.map(Into::into),
            repeat_index: repeat.map(|r| r.0),
            repeat_count: repeat.map(|r| r.1),
        }
    }

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../spec/fingerprint-vectors.json")).unwrap()
    }

    #[test]
    fn stable_attr_id_matches_the_shared_vectors() {
        let vectors = vectors();
        let cases = vectors["stableId"].as_array().unwrap();
        assert!(cases.len() >= 15);
        for case in cases {
            let id = case["id"].as_str().unwrap();
            assert_eq!(is_stable_attr_id(id), case["stable"].as_bool().unwrap(), "id {id:?}");
        }
    }

    #[test]
    fn ids_are_stable_and_shaped_as_documented() {
        let id = component_id("button", "add to cart", "main > list \"Products\" > listitem");
        assert_eq!(
            id,
            component_id("button", "add to cart", "main > list \"Products\" > listitem")
        );
        assert_eq!(id.len(), 14);
        assert!(id.starts_with("c_") && id[2..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            id,
            component_id("link", "add to cart", "main > list \"Products\" > listitem")
        );
    }

    #[test]
    fn names_that_look_the_same_normalize_the_same() {
        assert_eq!(normalize_name("  Add   to\nCart "), "add to cart");
        // "é" as one code point and as "e" plus a combining accent.
        assert_eq!(normalize_name("Caf\u{e9}"), normalize_name("Cafe\u{301}"));
        assert_eq!(normalize_name("कार्ट देखें"), "कार्ट देखें");
    }

    #[test]
    fn step_names_are_normalised_like_names() {
        let region = |name: &str| step("region", "section", Some(name), None);
        assert_eq!(step_key(&region("Cart  Preview")), step_key(&region("cart preview")));
        assert_eq!(step_key(&region("Caf\u{e9}")), step_key(&region("Cafe\u{301}")));
        assert_eq!(step_key(&region("  Cart\tPreview\n")), "region \"cart preview\"");
    }

    #[test]
    fn a_quote_or_delimiter_in_a_step_name_cannot_alias_another_path() {
        let join = |steps: &[PathStep]| steps.iter().map(step_key).collect::<Vec<_>>().join(" > ");
        let one = join(&[step("list", "ul", Some("a\" > list \"b"), None)]);
        let two = join(&[step("list", "ul", Some("a"), None), step("list", "ul", Some("b"), None)]);
        assert_ne!(one, two);
        assert_ne!(component_id("link", "x", &one), component_id("link", "x", &two));
        // A backslash cannot undo the escape of the quote after it.
        let slash = step_key(&step("list", "ul", Some("a\\"), None));
        let quote = step_key(&step("list", "ul", Some("a\""), None));
        assert_ne!(slash, quote);
    }

    #[test]
    fn stepkey_vectors() {
        let vectors = vectors();
        let cases = vectors["stepKey"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let step: PathStep = serde_json::from_value(case["step"].clone()).unwrap();
            assert_eq!(
                step_key(&step),
                case["key"].as_str().unwrap(),
                "step {:?}",
                case["step"]
            );
        }
    }

    #[test]
    fn frame_labels_are_escaped() {
        let vectors = vectors();
        let cases = vectors["frameStep"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(
                frame_step_key(case["label"].as_str().unwrap()),
                case["key"].as_str().unwrap()
            );
        }
        // A frame named like two steps cannot be mistaken for them.
        assert_ne!(
            frame_step_key("a\" > main \"b"),
            format!(
                "{} > {}",
                frame_step_key("a"),
                step_key(&step("main", "main", Some("b"), None))
            )
        );
    }

    #[test]
    fn path_starts_at_the_innermost_named_step() {
        let toc = |above: &[PathStep]| {
            let mut steps = above.to_vec();
            steps.push(step("navigation", "nav", Some("Contents"), None));
            steps.push(step("list", "ul", None, None));
            path_key(None, &steps)
        };
        let wide = toc(&[step("generic", "div", None, None), step("generic", "div", None, None)]);
        let narrow = toc(&[step("generic", "div", None, None), step("main", "main", None, None)]);
        assert_eq!(wide, "navigation \"contents\" > list");
        assert_eq!(wide, narrow);
        // Of two named steps, the inner one wins.
        let inner = path_key(
            None,
            &[
                step("main", "main", Some("Shop"), None),
                step("region", "section", Some("Cart"), None),
                step("generic", "div", None, None),
            ],
        );
        assert_eq!(inner, "region \"cart\" > div");
    }

    #[test]
    fn frame_step_is_always_kept() {
        let frame = frame_step_key("pay");
        let steps = [
            step("generic", "div", None, None),
            step("form", "form", Some("Card"), None),
        ];
        assert_eq!(path_key(Some(&frame), &steps), "frame \"pay\" > form \"card\"");
        assert_eq!(path_key(Some(&frame), &[]), "frame \"pay\"");
    }

    #[test]
    fn no_named_step_keeps_the_full_path() {
        let steps = [step("generic", "div", None, None), step("list", "ul", None, None)];
        assert_eq!(path_key(None, &steps), "div > list");
        assert_eq!(path_key(None, &[]), "");
    }

    #[test]
    fn repeated_step_names_dropped_by_the_repeat_rule_do_not_count_as_named() {
        // The article's name is data, so it is not named and the path keeps what is above it.
        let steps = [
            step("generic", "div", None, None),
            step("article", "article", Some("Blue Shirt"), Some((4, 12))),
        ];
        assert_eq!(path_key(None, &steps), "div > article");
        // A landmark keeps its name even when repeated, so it does count.
        let steps = [
            step("generic", "div", None, None),
            step("region", "section", Some("Founders"), Some((2, 5))),
            step("generic", "div", None, None),
        ];
        assert_eq!(path_key(None, &steps), "region \"founders\" > div");
    }

    #[test]
    fn pathkey_vectors() {
        let vectors = vectors();
        let cases = vectors["pathKey"].as_array().unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let steps: Vec<PathStep> = serde_json::from_value(case["steps"].clone()).unwrap();
            let frame = case["frame"].as_str();
            assert_eq!(path_key(frame, &steps), case["key"].as_str().unwrap(), "case {case}");
        }
    }

    #[test]
    fn repeated_steps_drop_what_varies() {
        assert_eq!(
            step_key(&step("list", "ul", Some("Products"), Some((0, 2)))),
            "list \"products\""
        );
        assert_eq!(
            step_key(&step("article", "article", Some("Blue Shirt"), Some((4, 12)))),
            "article"
        );
        assert_eq!(step_key(&step("generic", "label", None, Some((1, 3)))), "label");
        // A page with five named sections: the names are structure, not data.
        assert_eq!(
            step_key(&step("region", "section", Some("Founders"), Some((2, 5)))),
            "region \"founders\""
        );
    }
}
