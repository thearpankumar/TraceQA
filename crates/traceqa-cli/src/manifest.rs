//! The manifest format (`spec/manifest.schema.json`, `fixtures/buggy-shop/manifest.yaml`): what a
//! page should have. `traceqa baseline init` writes one from a finished run; this module builds it
//! and writes the YAML. Every page string is quoted by [`scalar`], so a hostile name cannot add keys.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use traceqa_protocol::types::{ComponentsFile, Confidence, ManifestComponent, ManifestFile, Viewport};

/// `ManifestFile.formatVersion`.
pub const FORMAT_VERSION: u64 = 1;

/// The name of a viewport: the one asked for with `--viewport-name`, else `WIDTHxHEIGHT`.
fn viewport_name(names: &BTreeMap<(u64, u64), String>, v: &Viewport) -> String {
    names
        .get(&(v.width, v.height))
        .cloned()
        .unwrap_or_else(|| format!("{}x{}", v.width, v.height))
}

/// The frame a component path starts with (`frame "newsletter" > ...`), unescaped.
pub fn frame_of(path: &str) -> Option<String> {
    let rest = path.strip_prefix("frame \"")?;
    let mut name = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => name.push(chars.next()?),
            '"' => return Some(name),
            c => name.push(c),
        }
    }
    None
}

/// Builds the manifest of a run from its finished targets.
///
/// Viewports are the distinct sizes of the targets, in order. A component is listed once per id
/// across all targets, in the order first seen. `instances` and `disabled` are the largest counts
/// seen; `hidden` lists the viewports where no instance is visible (or the component is not found),
/// judged by the first target at that viewport. `warnings` are the codes of every target.
pub fn build(files: &[ComponentsFile], names: &BTreeMap<(u64, u64), String>) -> ManifestFile {
    let mut viewports: Vec<(String, &ComponentsFile)> = Vec::new();
    for file in files {
        let name = viewport_name(names, &file.viewport);
        if !viewports.iter().any(|(n, _)| *n == name) {
            viewports.push((name, file));
        }
    }
    let mut order: Vec<&str> = Vec::new();
    for file in files {
        for c in &file.components {
            if !order.contains(&c.id.as_str()) {
                order.push(&c.id);
            }
        }
    }
    let mut components = Vec::new();
    for id in order {
        let all: Vec<_> = files
            .iter()
            .filter_map(|f| f.components.iter().find(|c| c.id == id))
            .collect();
        let first = all[0];
        let hidden: Vec<String> = viewports
            .iter()
            .filter(|(_, file)| {
                file.components
                    .iter()
                    .find(|c| c.id == id)
                    .is_none_or(|c| !c.instances.iter().any(|i| i.state.visible))
            })
            .map(|(name, _)| name.clone())
            .collect();
        let instances = all.iter().map(|c| c.instances.len()).max().unwrap_or(1) as u64;
        let disabled = all
            .iter()
            .map(|c| c.instances.iter().filter(|i| !i.state.enabled).count())
            .max()
            .unwrap_or(0) as u64;
        let low = all.iter().any(|c| c.confidence == Confidence::Low);
        components.push(ManifestComponent {
            case: None,
            role: first.role.clone(),
            name: (!first.name_is_template).then(|| first.name.clone()),
            template: first.name_is_template.then_some(true),
            instances: (instances != 1).then_some(instances),
            confidence: low.then_some(Confidence::Low),
            frame: frame_of(&first.path),
            shadow: all
                .iter()
                .any(|c| c.instances.iter().any(|i| i.in_shadow_dom))
                .then_some(true),
            hidden,
            disabled: (disabled > 0).then_some(disabled),
        });
    }
    let mut warnings: Vec<String> = files
        .iter()
        .flat_map(|f| f.warnings.iter().map(|w| w.code.clone()))
        .collect();
    warnings.sort();
    warnings.dedup();
    ManifestFile {
        format_version: Some(FORMAT_VERSION),
        viewports: viewports
            .iter()
            .map(|(name, file)| (name.clone(), file.viewport.clone()))
            .collect(),
        absent: Vec::new(),
        warnings,
        components,
    }
}

/// A YAML scalar for page text. Plain words stay plain, as in the hand-written manifest; anything
/// else, including words YAML 1.1 reads as another type, is double-quoted with JSON escapes plus
/// the line separators YAML (and not JSON) treats as breaks.
pub fn scalar(text: &str) -> String {
    let plain_chars = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-' | '\''))
    };
    let reserved = matches!(
        text.to_ascii_lowercase().as_str(),
        "y" | "n" | "yes" | "no" | "true" | "false" | "on" | "off" | "null"
    );
    let starts_ok = text.starts_with(|c: char| c.is_ascii_alphabetic());
    if starts_ok && plain_chars(text) && !text.ends_with(' ') && !text.contains("  ") && !reserved {
        return text.to_string();
    }
    let quoted = serde_json::to_string(text).expect("a string always serialises");
    quoted
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
        .replace('\u{85}', "\\u0085")
}

/// The manifest as YAML in the layout of `fixtures/buggy-shop/manifest.yaml`, after a header that
/// says where it came from.
pub fn to_yaml(manifest: &ManifestFile, source: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# What TraceQA should find on the page, written by `traceqa baseline init` from {source}.\n\
         # Review it: it records what was found, not what should be there. Fields per component:\n\
         #   role, name  what it is; `template: true` when instances have different names\n\
         #   instances   how many copies exist (default 1)\n\
         #   confidence  high (default) or low\n\
         #   frame       name of the iframe it lives in (default: the main page)\n\
         #   shadow      true when it is inside shadow DOM\n\
         #   hidden      viewports where it is not visible\n\
         #   disabled    how many instances are disabled (default 0)\n"
    );
    let _ = writeln!(
        out,
        "formatVersion: {}\n",
        manifest.format_version.unwrap_or(FORMAT_VERSION)
    );
    out.push_str("viewports:\n");
    let sorted: BTreeMap<&String, &Viewport> = manifest.viewports.iter().collect();
    let mut by_size: Vec<(&String, &Viewport)> = sorted.into_iter().collect();
    by_size.sort_by_key(|(name, v)| (std::cmp::Reverse(v.width), *name));
    for (name, v) in by_size {
        let _ = writeln!(
            out,
            "  {}: {{ width: {}, height: {} }}",
            scalar(name),
            v.width,
            v.height
        );
    }
    if !manifest.absent.is_empty() {
        out.push_str("\nabsent:\n");
        for a in &manifest.absent {
            let _ = writeln!(out, "  - {{ role: {}, name: {} }}", scalar(&a.role), scalar(&a.name));
        }
    }
    if !manifest.warnings.is_empty() {
        out.push_str("\nwarnings:\n");
        for w in &manifest.warnings {
            let _ = writeln!(out, "  - {}", scalar(w));
        }
    }
    out.push_str("\ncomponents:\n");
    for c in &manifest.components {
        let mut fields = vec![format!("role: {}", scalar(&c.role))];
        if let Some(name) = &c.name {
            fields.push(format!("name: {}", scalar(name)));
        }
        if c.template == Some(true) {
            fields.push("template: true".into());
        }
        if let Some(n) = c.instances {
            fields.push(format!("instances: {n}"));
        }
        if c.confidence == Some(Confidence::Low) {
            fields.push("confidence: low".into());
        }
        if let Some(frame) = &c.frame {
            fields.push(format!("frame: {}", scalar(frame)));
        }
        if c.shadow == Some(true) {
            fields.push("shadow: true".into());
        }
        if !c.hidden.is_empty() {
            let names: Vec<String> = c.hidden.iter().map(|h| scalar(h)).collect();
            fields.push(format!("hidden: [{}]", names.join(", ")));
        }
        if let Some(n) = c.disabled {
            fields.push(format!("disabled: {n}"));
        }
        let _ = writeln!(out, "  - {{ {} }}", fields.join(", "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = include_str!("../../../spec/examples/ComponentsFile.shop.json");

    fn shop() -> ComponentsFile {
        serde_json::from_str(SHOP).unwrap()
    }

    #[test]
    fn frames_are_read_from_the_path() {
        assert_eq!(frame_of("frame \"newsletter\" > form"), Some("newsletter".into()));
        assert_eq!(frame_of("frame \"a \\\"q\\\" b\" > x"), Some("a \"q\" b".into()));
        assert_eq!(frame_of("list \"products\""), None);
    }

    #[test]
    fn plain_words_stay_plain_and_the_rest_is_quoted() {
        assert_eq!(scalar("Buggy Shop"), "Buggy Shop");
        assert_eq!(scalar("Under $25"), "\"Under $25\"");
        assert_eq!(scalar("yes"), "\"yes\"");
        assert_eq!(scalar(""), "\"\"");
        assert_eq!(scalar("a: b"), "\"a: b\"");
        assert_eq!(scalar("x\u{2028}y"), "\"x\\u2028y\"");
    }

    #[test]
    fn the_yaml_parses_back_to_the_same_manifest() {
        let mut file = shop();
        file.components[0].name = "a: [b], {c} \"d\"\n# not a comment".into();
        let manifest = build(&[file], &BTreeMap::new());
        let yaml = to_yaml(&manifest, "run r_test");
        let parsed: ManifestFile = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(parsed.components.len(), manifest.components.len());
        assert_eq!(parsed.components[0].name, manifest.components[0].name);
        assert_eq!(parsed.viewports, manifest.viewports);
    }

    #[test]
    fn hidden_and_disabled_come_from_instance_state() {
        let mut file = shop();
        for i in &mut file.components[0].instances {
            i.state.visible = false;
            i.state.enabled = true;
        }
        file.components[0].instances[0].state.enabled = false;
        let manifest = build(&[file], &BTreeMap::from([((1280, 800), "desktop".to_string())]));
        let c = &manifest.components[0];
        assert_eq!(c.hidden, ["desktop"]);
        assert_eq!(c.disabled, Some(1));
        assert!(manifest.viewports.contains_key("desktop"));
    }
}
