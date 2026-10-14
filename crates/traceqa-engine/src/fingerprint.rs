//! The component id. The recipe is specified in `spec/fingerprint.md`; change them together.

use sha2::{Digest, Sha256};
use traceqa_protocol::types::PathStep;
use unicode_normalization::UnicodeNormalization;

pub const VERSION: u64 = 1;

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

/// One step of a path key: the role (the tag when there is no role), plus the name in quotes.
/// A repeated step that is not a landmark loses its name, because the name of one card among
/// many is data. Indexes are always left out, so every card in a grid has the same path.
pub fn step_key(step: &PathStep) -> String {
    let base = if step.role == "generic" { &step.tag } else { &step.role };
    let is_data = step.repeat_count.unwrap_or(0) >= REPEAT_THRESHOLD && !LANDMARKS.contains(&step.role.as_str());
    match &step.name {
        Some(name) if !is_data => format!("{base} \"{name}\""),
        _ => base.clone(),
    }
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
    fn repeated_steps_drop_what_varies() {
        assert_eq!(
            step_key(&step("list", "ul", Some("Products"), Some((0, 2)))),
            "list \"Products\""
        );
        assert_eq!(
            step_key(&step("article", "article", Some("Blue Shirt"), Some((4, 12)))),
            "article"
        );
        assert_eq!(step_key(&step("generic", "label", None, Some((1, 3)))), "label");
        // A page with five named sections: the names are structure, not data.
        assert_eq!(
            step_key(&step("region", "section", Some("Founders"), Some((2, 5)))),
            "region \"Founders\""
        );
    }
}
