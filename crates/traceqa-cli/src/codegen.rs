//! `traceqa codegen`: a finished run in, a Playwright page object out.
//!
//! The run already holds, for every component, how to find it again (`Component.locator`, worked
//! out by the engine for one browser). This command reads the files of all the run's browsers,
//! decides what holds for all of them together, and writes one class with one property per
//! component, in TypeScript or Python. It computes nothing about the page itself, so it stays a
//! thin consumer of the identity layer.
//!
//! Page text (names, test ids, frame labels, CSS) ends up inside source code. It is written only
//! through `traceqa_common::locator::{ts_string, py_string}`, even inside comments, and
//! names that become identifiers are cut down to ASCII letters and digits.

mod py;
mod ts;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use serde_json::{json, Value};
use traceqa_common::types::{Component, ComponentsFile, Locator, LocatorStrategy, RunManifest, RunStatus};

#[derive(Clone, Copy, ValueEnum)]
enum Lang {
    Ts,
    Py,
}

#[derive(Args)]
pub struct CodegenArgs {
    /// Run folder made by `traceqa extract`.
    run: PathBuf,
    /// Language of the page object.
    #[arg(long, value_enum)]
    lang: Lang,
    /// Folder to write into.
    #[arg(long, default_value = "./traceqa-pages")]
    out: PathBuf,
    /// Use only this target of the run: its folder name (chromium-1280x800) or its browser name
    /// (chromium). Repeat for several. Default: every completed target.
    #[arg(long = "target", value_name = "NAME")]
    targets: Vec<String>,
    /// Name of the generated class. Default: made from the page title, for example BuggyShopPage.
    #[arg(long, value_name = "NAME")]
    class_name: Option<String>,
    /// Also write components that are hidden in every target. They are found with includeHidden.
    /// Default: leave them out, because a locator for something nobody can see matches nothing.
    #[arg(long)]
    include_hidden: bool,
}

pub fn run(args: &CodegenArgs) -> ExitCode {
    let plan = match Plan::read(args) {
        Ok(plan) => plan,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    let (file_name, source) = match args.lang {
        Lang::Ts => (ts::file_name(&plan), ts::render(&plan)),
        Lang::Py => (py::file_name(&plan), py::render(&plan)),
    };
    let written = std::fs::create_dir_all(&args.out)
        .and_then(|()| std::fs::write(args.out.join(&file_name), source))
        .and_then(|()| {
            let text = serde_json::to_string_pretty(&plan.locators_json(args.lang)).expect("a Value always serialises");
            std::fs::write(args.out.join("locators.json"), text + "\n")
        });
    if let Err(error) = written {
        eprintln!("traceqa: could not write into {}: {error}", args.out.display());
        return ExitCode::from(2);
    }
    let unique = plan.entries.iter().filter(|e| e.unique).count();
    let forced = plan.entries.iter().filter(|e| e.locator.forced).count();
    let multi = plan.entries.iter().filter(|e| e.multi > 1).count();
    println!(
        "Wrote {} properties to {} ({unique} unique, {multi} match several elements, {forced} forced by CSS, {} left out as hidden)",
        plan.entries.len(),
        args.out.join(&file_name).display(),
        plan.skipped.len(),
    );
    println!("Wrote {}", args.out.join("locators.json").display());
    ExitCode::SUCCESS
}

/// One property of the page object.
pub(crate) struct Entry {
    pub id: String,
    /// Lower-case ASCII words the identifier is made of.
    pub words: Vec<String>,
    /// First hex digits of the id when the words alone would clash with another property.
    pub suffix: Option<String>,
    pub role: String,
    pub name: String,
    /// What holds in every target that has the component.
    pub locator: Locator,
    /// Unique in every target that has it, with the same descriptor in all of them.
    pub unique: bool,
    /// The targets disagreed about how to find it; the locator with the most scope was taken.
    pub differs: bool,
    /// The most elements it matched in any target.
    pub multi: u64,
    pub present_in: Vec<String>,
    pub unique_in: Vec<String>,
}

impl Entry {
    /// `addToCartButton`, with the id's first hex digits after an underscore when needed.
    pub fn ident_ts(&self) -> String {
        let mut out = String::new();
        for (i, word) in self.words.iter().enumerate() {
            if i == 0 {
                out.push_str(word);
            } else {
                let mut chars = word.chars();
                out.extend(chars.next().map(|c| c.to_ascii_uppercase()));
                out.push_str(chars.as_str());
            }
        }
        if let Some(suffix) = &self.suffix {
            out.push('_');
            out.push_str(suffix);
        }
        out
    }

    /// `add_to_cart_button`, with the id's first hex digits after an underscore when needed.
    pub fn ident_py(&self) -> String {
        let mut out = self.words.join("_");
        if let Some(suffix) = &self.suffix {
            out.push('_');
            out.push_str(suffix);
        }
        out
    }
}

pub(crate) struct Skipped {
    pub id: String,
    pub role: String,
    pub name: String,
}

pub(crate) struct Plan {
    pub class_name: String,
    /// The file name without its extension part, for example `buggy-shop`.
    pub stem: String,
    pub url: String,
    pub run_id: String,
    pub targets: Vec<String>,
    pub entries: Vec<Entry>,
    pub skipped: Vec<Skipped>,
}

/// A component id is `c_` and twelve hex digits. Anything else in a file is not an id.
pub(crate) fn is_component_id(id: &str) -> bool {
    id.len() == 14 && id.starts_with("c_") && id[2..].bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Words made of the ASCII letters and digits in `text`, lower case. Everything else
/// separates words, so nothing from the page can reach an identifier except these.
fn ascii_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// At most this many words of a name go into an identifier.
const MAX_WORDS: usize = 4;
const MAX_IDENT_CHARS: usize = 40;

/// The words of a property's identifier: what it is called, then what it is.
fn identifier_words(component: &Component, locator: &Locator) -> Vec<String> {
    let label = if !component.name_is_template && component.name_changes != Some(true) && !component.name.is_empty() {
        component.name.clone()
    } else if let Some(id) = &locator.test_id {
        id.clone()
    } else {
        locator
            .scope
            .iter()
            .rev()
            .find_map(|scope| scope.name.clone())
            .unwrap_or_default()
    };
    let mut words: Vec<String> = Vec::new();
    let mut chars = 0;
    for word in ascii_words(&label).into_iter().take(MAX_WORDS) {
        chars += word.len();
        if chars > MAX_IDENT_CHARS {
            break;
        }
        words.push(word);
    }
    let kind = match (locator.strategy, ascii_words(&component.role).first()) {
        (LocatorStrategy::Text, _) | (_, None) => "text".to_string(),
        (_, Some(role)) if role == "generic" => "element".to_string(),
        (_, Some(role)) => role.clone(),
    };
    words.push(kind);
    if words[0].starts_with(|c: char| c.is_ascii_digit()) {
        words.insert(0, "el".to_string());
    }
    words
}

const RESERVED: [&str; 5] = ["page", "constructor", "prototype", "then", "init"];

/// Ranks descriptors for "the safer one": more ancestors first, then the more readable strategy.
fn safety(locator: &Locator) -> (usize, u8) {
    let rank = match locator.strategy {
        LocatorStrategy::Role => 3,
        LocatorStrategy::Testid => 2,
        LocatorStrategy::Text => 1,
        LocatorStrategy::Css => 0,
    };
    (locator.scope.len(), rank)
}

/// The parts of a locator that say how to find the element, without the counts that depend on
/// the one page it was measured on and without `needsIncludeHidden` (decided by the whole run).
fn descriptor(locator: &Locator) -> Locator {
    Locator {
        matches: 0,
        own: 0,
        unique: false,
        ambiguous_with: Vec::new(),
        needs_include_hidden: false,
        ..locator.clone()
    }
}

struct Loaded {
    dir: String,
    file: ComponentsFile,
}

/// A name from the manifest that is used as a folder name must be one plain folder.
fn is_plain_folder_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) && !name.contains(['/', '\\'])
}

impl Plan {
    fn read(args: &CodegenArgs) -> Result<Plan, String> {
        let manifest_path = args.run.join("run.json");
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            format!(
                "{} is not a run folder: could not read run.json ({e}). Make one with `traceqa extract URL`.",
                args.run.display()
            )
        })?;
        let manifest: RunManifest = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a run manifest: {e}", manifest_path.display()))?;

        let mut loaded: Vec<Loaded> = Vec::new();
        for target in &manifest.targets {
            if target.status != RunStatus::Completed {
                continue;
            }
            let wanted = args.targets.is_empty()
                || args.targets.iter().any(|name| {
                    *name == target.dir
                        || target.browser.as_ref().is_some_and(|b| *name == b.name)
                        || *name == target.target.browser
                });
            if !wanted {
                continue;
            }
            if !is_plain_folder_name(&target.dir) {
                return Err(format!(
                    "run.json names a target folder {:?} that is not a plain folder name",
                    target.dir
                ));
            }
            let path = args.run.join(&target.dir).join("components.json");
            let text = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let file: ComponentsFile =
                serde_json::from_str(&text).map_err(|e| format!("{} is not a components file: {e}", path.display()))?;
            loaded.push(Loaded {
                dir: target.dir.clone(),
                file,
            });
        }
        if loaded.is_empty() {
            let known: Vec<&str> = manifest.targets.iter().map(|t| t.dir.as_str()).collect();
            return Err(if args.targets.is_empty() {
                "the run has no completed target to read".to_string()
            } else {
                format!(
                    "no completed target matches --target {}. The run has: {}",
                    args.targets.join(", "),
                    known.join(", ")
                )
            });
        }
        if let Some(file) = loaded
            .iter()
            .find(|l| l.file.components.iter().any(|c| c.locator.is_none()))
        {
            return Err(format!(
                "{} has components without a locator: the run was made by an older traceqa. Extract the page again.",
                file.dir
            ));
        }

        let (class_name, stem) = names(args.class_name.as_deref(), &loaded[0].file.title)?;
        Ok(merge(
            &loaded,
            class_name,
            stem,
            manifest.url.clone(),
            manifest.run_id.clone(),
            args.include_hidden,
        ))
    }

    fn locators_json(&self, lang: Lang) -> Value {
        let mut components = serde_json::Map::new();
        for entry in &self.entries {
            components.insert(
                entry.id.clone(),
                json!({
                    "property": match lang { Lang::Ts => entry.ident_ts(), Lang::Py => entry.ident_py() },
                    "role": entry.role,
                    "name": entry.name,
                    "locator": entry.locator,
                    "presentIn": entry.present_in,
                    "uniqueIn": entry.unique_in,
                    "unique": entry.unique,
                    "forced": entry.locator.forced,
                    "differsAcrossTargets": entry.differs,
                    "matchesAtMost": entry.multi,
                }),
            );
        }
        json!({
            "formatVersion": 1,
            "runId": self.run_id,
            "url": self.url,
            "language": match lang { Lang::Ts => "ts", Lang::Py => "py" },
            "className": self.class_name,
            "targets": self.targets,
            "components": components,
            "leftOutAsHidden": self.skipped.iter().map(|s| json!({"id": s.id, "role": s.role, "name": s.name})).collect::<Vec<_>>(),
        })
    }
}

/// The class name and the file stem: given, or made from the title.
fn names(given: Option<&str>, title: &str) -> Result<(String, String), String> {
    let class_name = match given {
        Some(name) => {
            let mut chars = name.chars();
            let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            if !first_ok || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!(
                    "--class-name {name:?} is not a name: use ASCII letters, digits and _, not starting with a digit"
                ));
            }
            name.to_string()
        }
        None => {
            let words = ascii_words(title);
            if words.is_empty() {
                "AppPage".to_string()
            } else {
                let mut name: String = words
                    .iter()
                    .take(MAX_WORDS)
                    .map(|word| {
                        let mut chars = word.chars();
                        chars
                            .next()
                            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                            .unwrap_or_default()
                    })
                    .collect();
                if name.starts_with(|c: char| c.is_ascii_digit()) {
                    name.insert_str(0, "Page");
                }
                name.push_str("Page");
                name
            }
        }
    };
    let base = class_name
        .strip_suffix("Page")
        .filter(|b| !b.is_empty())
        .unwrap_or(&class_name);
    // CamelCase and snake_case both become lower-case words joined by dashes.
    let mut stem = String::new();
    let mut previous_lower = false;
    for c in base.chars() {
        if c.is_ascii_uppercase() && previous_lower {
            stem.push('-');
        }
        stem.push(if c == '_' { '-' } else { c.to_ascii_lowercase() });
        previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    Ok((class_name, stem))
}

fn merge(
    loaded: &[Loaded],
    class_name: String,
    stem: String,
    url: String,
    run_id: String,
    include_hidden: bool,
) -> Plan {
    let targets: Vec<String> = loaded.iter().map(|l| l.dir.clone()).collect();

    // Each component id with its appearances, in order of first appearance.
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashMap<String, Vec<(&str, &Component)>> = HashMap::new();
    for l in loaded {
        for component in &l.file.components {
            if !is_component_id(&component.id) {
                continue;
            }
            seen.entry(component.id.clone())
                .or_insert_with(|| {
                    order.push(component.id.clone());
                    Vec::new()
                })
                .push((l.dir.as_str(), component));
        }
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    for id in order {
        let appearances = &seen[&id];
        let hidden_everywhere = appearances
            .iter()
            .all(|(_, c)| !c.instances.is_empty() && c.instances.iter().all(|i| !i.state.visible));
        let (_, first) = appearances[0];
        if hidden_everywhere && !include_hidden {
            skipped.push(Skipped {
                id,
                role: first.role.clone(),
                name: first.name.clone(),
            });
            continue;
        }
        let locators: Vec<&Locator> = appearances.iter().filter_map(|(_, c)| c.locator.as_ref()).collect();
        let mut best = locators[0];
        for locator in &locators[1..] {
            if safety(locator) > safety(best) {
                best = locator;
            }
        }
        let differs = locators.iter().any(|l| descriptor(l) != descriptor(best));
        let mut merged = best.clone();
        merged.unique = !differs && locators.iter().all(|l| l.unique);
        merged.needs_include_hidden = locators.iter().all(|l| l.needs_include_hidden);
        merged.matches = locators.iter().map(|l| l.matches).max().unwrap_or(0);
        merged.own = locators.iter().map(|l| l.own).max().unwrap_or(0);
        let mut also: Vec<String> = Vec::new();
        for other in locators.iter().flat_map(|l| &l.ambiguous_with) {
            if is_component_id(other) && !also.contains(other) && also.len() < 5 {
                also.push(other.clone());
            }
        }
        merged.ambiguous_with = also;
        entries.push(Entry {
            words: identifier_words(first, &merged),
            suffix: None,
            role: first.role.clone(),
            name: first.name.clone(),
            unique: merged.unique,
            multi: appearances
                .iter()
                .map(|(_, c)| c.instances.len() as u64)
                .max()
                .unwrap_or(0),
            present_in: appearances.iter().map(|(dir, _)| (*dir).to_string()).collect(),
            unique_in: appearances
                .iter()
                .filter(|(_, c)| c.locator.as_ref().is_some_and(|l| l.unique))
                .map(|(dir, _)| (*dir).to_string())
                .collect(),
            locator: merged,
            differs,
            id,
        });
    }

    // Names that would clash, or take a member's place, get the first hex digits of the id.
    // Every member of a clash gets one, so a name does not depend on which came first.
    let mut taken: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &entries {
        *taken.entry(entry.ident_py()).or_default() += 1;
    }
    for entry in &mut entries {
        let ident = entry.ident_py();
        if taken[&ident] > 1 || RESERVED.contains(&ident.as_str()) {
            entry.suffix = Some(entry.id[2..8].to_string());
        }
    }
    // Six digits are enough unless two ids start alike; then the whole id is used.
    let mut again: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &entries {
        *again.entry(entry.ident_py()).or_default() += 1;
    }
    for entry in &mut entries {
        if again[&entry.ident_py()] > 1 {
            entry.suffix = Some(entry.id[2..].to_string());
        }
    }

    Plan {
        class_name,
        stem,
        url,
        run_id,
        targets,
        entries,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator(strategy: LocatorStrategy) -> Locator {
        Locator {
            strategy,
            role: None,
            name: None,
            exact: None,
            test_id: None,
            scope: Vec::new(),
            frame: None,
            css: None,
            unique: true,
            matches: 1,
            own: 1,
            ambiguous_with: Vec::new(),
            forced: false,
            needs_include_hidden: false,
        }
    }

    fn component(id: &str, role: &str, name: &str, locator: Locator) -> Component {
        Component {
            id: id.into(),
            role: role.into(),
            name: name.into(),
            name_is_template: false,
            name_changes: None,
            confidence: traceqa_common::types::Confidence::High,
            signals: Vec::new(),
            path: String::new(),
            crop: None,
            locator: Some(locator),
            instances: Vec::new(),
        }
    }

    #[test]
    fn identifiers_keep_only_ascii_words() {
        let c = component(
            "c_aaaaaaaaaaaa",
            "button",
            "Add to cart!",
            locator(LocatorStrategy::Role),
        );
        assert_eq!(
            identifier_words(&c, &c.locator.clone().unwrap()),
            ["add", "to", "cart", "button"]
        );
        let hindi = component("c_aaaaaaaaaaab", "button", "कार्ट देखें", locator(LocatorStrategy::Role));
        assert_eq!(identifier_words(&hindi, &hindi.locator.clone().unwrap()), ["button"]);
        let hostile = component(
            "c_aaaaaaaaaaac",
            "link",
            "\"); rm -rf /; (\"",
            locator(LocatorStrategy::Role),
        );
        assert_eq!(
            identifier_words(&hostile, &hostile.locator.clone().unwrap()),
            ["rm", "rf", "link"]
        );
        let digit = component("c_aaaaaaaaaaad", "link", "24 hour help", locator(LocatorStrategy::Role));
        assert_eq!(identifier_words(&digit, &digit.locator.clone().unwrap())[0], "el");
    }

    #[test]
    fn class_names_come_from_the_title_or_are_checked() {
        assert_eq!(
            names(None, "Buggy Shop").unwrap(),
            ("BuggyShopPage".into(), "buggy-shop".into())
        );
        assert_eq!(names(None, "").unwrap(), ("AppPage".into(), "app".into()));
        assert_eq!(names(None, "कार्ट").unwrap().0, "AppPage");
        assert_eq!(
            names(Some("CheckoutPage"), "x").unwrap(),
            ("CheckoutPage".into(), "checkout".into())
        );
        assert!(names(Some("a b"), "x").is_err());
        assert!(names(Some("1Page"), "x").is_err());
        assert!(names(Some("A\"; evil"), "x").is_err());
    }

    #[test]
    fn plain_folder_names_only() {
        assert!(is_plain_folder_name("chromium-1280x800"));
        for bad in ["..", "../x", "a/b", "a\\b", "", ".", "/abs"] {
            assert!(!is_plain_folder_name(bad), "{bad}");
        }
    }

    #[test]
    fn component_ids_are_checked() {
        assert!(is_component_id("c_0123456789ab"));
        assert!(!is_component_id("c_0123456789aB"));
        assert!(!is_component_id("c_0123456789a\n"));
        assert!(!is_component_id("x_0123456789ab"));
    }

    fn file(title: &str, components: &[Component]) -> ComponentsFile {
        serde_json::from_value(json!({
            "formatVersion": 1, "fingerprintVersion": 2, "runId": "r", "url": "u", "finalUrl": "u",
            "title": title, "browser": {"name": "x", "version": "1"},
            "viewport": {"width": 1, "height": 1}, "capturedAt": "t", "warnings": [],
            "components": components,
        }))
        .expect("a components file")
    }

    fn plan_of(files: Vec<(&str, Vec<Component>)>, include_hidden: bool) -> Plan {
        let loaded: Vec<Loaded> = files
            .into_iter()
            .map(|(dir, components)| Loaded {
                dir: dir.into(),
                file: file("Shop", &components),
            })
            .collect();
        merge(
            &loaded,
            "ShopPage".into(),
            "shop".into(),
            "u".into(),
            "r".into(),
            include_hidden,
        )
    }

    #[test]
    fn a_component_is_unique_only_if_unique_in_every_target() {
        let mut ambiguous = locator(LocatorStrategy::Role);
        ambiguous.unique = false;
        let plan = plan_of(
            vec![
                (
                    "a",
                    vec![component(
                        "c_000000000001",
                        "button",
                        "Go",
                        locator(LocatorStrategy::Role),
                    )],
                ),
                ("b", vec![component("c_000000000001", "button", "Go", ambiguous)]),
            ],
            false,
        );
        let (a, b) = ("a".to_string(), "b".to_string());
        let entry = &plan.entries[0];
        assert!(!entry.unique);
        assert!(!entry.differs, "same descriptor, only the counts differ");
        assert_eq!(entry.unique_in, std::slice::from_ref(&a));
        assert_eq!(entry.present_in, [a, b]);
    }

    #[test]
    fn differing_descriptors_take_the_one_with_more_scope() {
        let plain = locator(LocatorStrategy::Role);
        let mut scoped = locator(LocatorStrategy::Role);
        scoped.scope = vec![traceqa_common::types::LocatorScope {
            role: "navigation".into(),
            name: None,
        }];
        let plan = plan_of(
            vec![
                ("a", vec![component("c_000000000001", "link", "Home", plain)]),
                ("b", vec![component("c_000000000001", "link", "Home", scoped)]),
            ],
            false,
        );
        let entry = &plan.entries[0];
        assert!(entry.differs && !entry.unique);
        assert_eq!(entry.locator.scope.len(), 1);
    }

    #[test]
    fn clashing_names_get_the_id_prefix_on_every_member() {
        let mk = |id: &str| component(id, "link", "Home", locator(LocatorStrategy::Role));
        let plan = plan_of(
            vec![(
                "a",
                vec![
                    mk("c_aaaaaa000001"),
                    mk("c_bbbbbb000002"),
                    component("c_cccccc000003", "button", "Go", locator(LocatorStrategy::Role)),
                ],
            )],
            false,
        );
        let idents: Vec<String> = plan.entries.iter().map(Entry::ident_ts).collect();
        assert_eq!(idents, ["homeLink_aaaaaa", "homeLink_bbbbbb", "goButton"]);
        assert_eq!(plan.entries[0].ident_py(), "home_link_aaaaaa");
    }

    #[test]
    fn hidden_components_are_left_out_unless_asked_for() {
        // An instance-less component is not "hidden"; build one hidden instance by JSON.
        let mut hidden = component("c_000000000001", "button", "Menu", locator(LocatorStrategy::Role));
        hidden.instances = serde_json::from_value(json!([{
            "key": "k", "name": "Menu", "tag": "button", "attrs": {}, "selectors": [], "frameId": "main",
            "inShadowDom": false, "box": {"x": 0, "y": 0, "w": 1, "h": 1},
            "state": {"visible": false, "enabled": true, "inViewport": false}
        }]))
        .expect("an instance");
        let left_out = plan_of(vec![("a", vec![hidden.clone()])], false);
        assert!(left_out.entries.is_empty());
        assert_eq!(left_out.skipped.len(), 1);
        let kept = plan_of(vec![("a", vec![hidden])], true);
        assert_eq!(kept.entries.len(), 1);
    }
}
