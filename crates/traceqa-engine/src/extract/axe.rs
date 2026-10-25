//! Optional accessibility checks with axe-core, placed on the components of the page.
//!
//! axe-core is MPL-2.0 and is not part of TraceQA: `traceqa install --axe` downloads it from the
//! npm registry into `$TRACEQA_HOME/axe/<version>/`, and a run that asks for it reads that copy.
//! Nothing here embeds it. The file is sent through the driver's `page.eval` as part of an
//! ordinary function (never through the page's own `eval`, which a Content-Security-Policy can
//! block), once per frame the driver reaches. In the page, a small wrapper runs `axe.run`, drops
//! the html axe reports and measures each failing element's box the way the collector does. Which
//! component an element belongs to is decided here, in plain Rust, from boxes alone.
//!
//! A target where axe cannot run (a driver that cannot wait for a Promise, a timeout, a frame
//! that refuses scripts) gets the warning `axe_failed` and still completes.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use traceqa_common::types::{
    AxeNode, AxeResults, AxeViolation, Component, Instance, PageEvalParams, Rect, SessionOpenResult, Warning,
};

use super::frames::offset;
use super::{warning, Job};
use crate::driver::{Driver, DEFAULT_TIMEOUT_MS};
use crate::home;

/// The axe-core version `traceqa install --axe` downloads, and the one a run looks for.
pub const PINNED_VERSION: &str = "4.14.0";
/// Where the pinned version comes from: the official npm registry.
pub const TARBALL_URL: &str = "https://registry.npmjs.org/axe-core/-/axe-core-4.14.0.tgz";
/// The registry's `dist.integrity` of that tarball (SHA-512, base64). The installer refuses any other bytes.
pub const TARBALL_INTEGRITY_SHA512: &str =
    "9WTZxEjsZ7b13TH8JPmbV2z8CHbl80/2hm3XPEG4JgNdQLK81IBRXmSxHfMAOkSqQeRxT/0dwNDz2GOm3zzpcQ==";
/// Where the source of axe-core lives.
pub const SOURCE_URL: &str = "https://github.com/dequelabs/axe-core";
/// Names a copy of `axe.min.js` to use instead of the installed one (`--axe-file` sets it).
pub const FILE_ENV: &str = "TRACEQA_AXE_FILE";

/// The rule tags that are run: every WCAG level through 2.2 AA, and axe's best practices.
pub const TAGS: [&str; 6] = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"];

/// Largest `axe.min.js` accepted. The real file is about 650 KB.
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
/// Failing elements kept per rule. A page with thousands would make a file nobody reads.
const MAX_NODES_PER_RULE: usize = 200;

/// Where the pinned copy is installed under TraceQA's folder.
pub fn installed_dir(home: &Path) -> PathBuf {
    home.join("axe").join(PINNED_VERSION)
}

/// The `axe.min.js` a run would use: the file `TRACEQA_AXE_FILE` names, else the installed copy.
pub fn locate() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(FILE_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(named));
    }
    let installed = installed_dir(&home::dir()?).join("axe.min.js");
    installed.is_file().then_some(installed)
}

/// axe-core's source, read from disk and ready to send to a page.
#[derive(Clone)]
pub struct AxeScript {
    source: String,
}

impl AxeScript {
    /// Reads the file `locate` finds. The error says what to do about it.
    pub fn load() -> Result<Self, String> {
        let path = locate().ok_or_else(|| {
            "--axe needs axe-core, which TraceQA does not ship (it is MPL-2.0). Run `traceqa install --axe` to \
             download the pinned copy from the npm registry, or give a copy with --axe-file PATH"
                .to_string()
        })?;
        Self::from_file(&path)
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let meta = fs::metadata(path).map_err(|e| format!("cannot read the axe-core file {}: {e}", path.display()))?;
        if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
            return Err(format!(
                "{} is not a usable axe-core file (it must be axe.min.js, at most {} MiB)",
                path.display(),
                MAX_SOURCE_BYTES / (1024 * 1024)
            ));
        }
        let source = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Ok(Self { source })
    }

    /// A function expression for `page.eval`. axe's source sits inside it, so it runs where the
    /// driver runs scripts and needs no `eval` in the page.
    fn function(&self) -> String {
        format!("async (tags) => {{\n{}\n;\n{WRAPPER}\n}}", self.source)
    }
}

/// What runs after axe's own source, in the same function.
const WRAPPER: &str = r#"
const tq = window.axe;
if (!tq || typeof tq.run !== "function") throw new Error("axe-core did not load in this frame");
const cap = (text, n) => String(text == null ? "" : text).slice(0, n);
const round = (n) => Math.round(n * 10) / 10;
const flat = (target) => (Array.isArray(target) ? target : [target]).map((t) => (Array.isArray(t) ? t.join(" ") : String(t))).join(" >> ");
let outcome;
try {
  outcome = await tq.run(document, {
    runOnly: { type: "tag", values: tags },
    resultTypes: ["violations", "incomplete"],
    iframes: false,
    elementRef: true,
  });
} finally {
  try { delete window.axe; } catch (e) { window.axe = undefined; }
}
const sx = window.scrollX, sy = window.scrollY;
const place = (node) => {
  let box = { x: 0, y: 0, w: 0, h: 0 }, tag = "";
  const el = node.element;
  if (el && typeof el.getBoundingClientRect === "function") {
    const r = el.getBoundingClientRect();
    box = { x: round(r.left + sx), y: round(r.top + sy), w: round(r.width), h: round(r.height) };
    tag = cap(el.localName, 40);
  }
  return { target: cap(flat(node.target), 300), box, tag };
};
return {
  version: cap(tq.version, 40),
  incomplete: outcome.incomplete.reduce((sum, rule) => sum + rule.nodes.length, 0),
  violations: outcome.violations.map((rule) => ({
    id: cap(rule.id, 100),
    impact: rule.impact ? cap(rule.impact, 20) : null,
    tags: rule.tags.slice(0, 30).map((t) => cap(t, 40)),
    help: cap(rule.help, 300),
    helpUrl: cap(rule.helpUrl, 300),
    total: rule.nodes.length,
    nodes: rule.nodes.slice(0, 200).map(place),
  })),
};"#;

/// What the wrapper returns for one frame.
#[derive(Deserialize)]
struct Reported {
    version: String,
    incomplete: u64,
    violations: Vec<ReportedRule>,
}

#[derive(Deserialize)]
struct ReportedRule {
    id: String,
    impact: Option<String>,
    tags: Vec<String>,
    help: String,
    #[serde(rename = "helpUrl")]
    help_url: String,
    total: u64,
    nodes: Vec<ReportedNode>,
}

#[derive(Deserialize)]
struct ReportedNode {
    target: String,
    #[serde(rename = "box")]
    box_: Rect,
    tag: String,
}

/// Runs axe in the page and its frames, writes `axe.json` next to `components.json` and adds
/// a warning when axe could not run everywhere. Nothing here can fail the target.
pub(super) fn run(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    components: &[Component],
    warnings: &mut Vec<Warning>,
) {
    let Some(script) = job.axe else {
        return;
    };
    match check(driver, session, job, script, components) {
        Ok((results, problems)) => {
            if let Err(error) = traceqa_recorder::write_axe(job.dir, &results) {
                warnings.push(warning(
                    "axe_failed",
                    format!("axe-core ran but axe.json could not be written: {error}"),
                ));
            } else if !problems.is_empty() {
                warnings.push(warning(
                    "axe_failed",
                    format!("axe-core did not run everywhere: {}", problems.join("; ")),
                ));
            }
        }
        Err(reason) => warnings.push(warning("axe_failed", format!("axe-core did not run: {reason}"))),
    }
}

fn check(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    script: &AxeScript,
    components: &[Component],
) -> Result<(AxeResults, Vec<String>), String> {
    if session.capabilities.eval_async != Some(true) {
        return Err(
            "this driver cannot wait for a script that returns a Promise (capability evalAsync), which axe-core needs"
                .into(),
        );
    }
    let evaluated = driver
        .until(job.cancel)
        .eval(&PageEvalParams {
            session_id: session.session_id.clone(),
            script: script.function(),
            arg: Some(json!(TAGS)),
            all_frames: Some(session.capabilities.eval_all_frames),
            await_promise: Some(true),
            timeout_ms: Some(job.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
        })
        .map_err(|error| error.to_string())?;

    let mut frames = Vec::new();
    let mut problems = Vec::new();
    for result in evaluated.results {
        let label = if result.frame.id == "main" {
            "the page".to_string()
        } else {
            format!("frame {}", result.frame.id)
        };
        let parsed = match result.value {
            Some(value) => serde_json::from_value::<Reported>(value).map_err(|e| format!("unreadable result: {e}")),
            None => Err(result.error.unwrap_or_else(|| "the driver gave no reason".into())),
        };
        match parsed {
            Ok(reported) => frames.push((result.frame.id, result.frame.box_, reported)),
            Err(reason) => problems.push(format!("{label}: {}", short(&reason))),
        }
    }
    if frames.is_empty() {
        return Err(problems.join("; "));
    }
    Ok((assemble(frames, components, &mut problems), problems))
}

/// Keeps a driver's or page's error text to one readable line.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    line.chars().take(200).collect()
}

/// Joins the frames' reports into one file, with every node placed on a component.
fn assemble(
    frames: Vec<(String, Option<Rect>, Reported)>,
    components: &[Component],
    problems: &mut Vec<String>,
) -> AxeResults {
    let mut version = String::new();
    let mut incomplete = 0;
    let mut violations: Vec<AxeViolation> = Vec::new();
    for (frame_id, frame_box, reported) in frames {
        if version.is_empty() {
            version = reported.version;
        }
        incomplete += reported.incomplete;
        for rule in reported.violations {
            if rule.total > rule.nodes.len() as u64 {
                problems.push(format!(
                    "{}: {} elements failed in frame {frame_id}; the first {} are kept",
                    rule.id,
                    rule.total,
                    rule.nodes.len().min(MAX_NODES_PER_RULE)
                ));
            }
            let nodes = rule.nodes.into_iter().take(MAX_NODES_PER_RULE).map(|node| {
                let page_box = frame_box
                    .as_ref()
                    .map_or_else(|| node.box_.clone(), |at| offset(&node.box_, at));
                let placed = place(&frame_id, &page_box, &node.tag, components);
                AxeNode {
                    frame_id: frame_id.clone(),
                    target: node.target,
                    box_: page_box,
                    component_id: placed.as_ref().map(|(id, _)| id.clone()),
                    via_ancestor: placed.map(|(_, via_ancestor)| via_ancestor).filter(|via| *via),
                }
            });
            match violations.iter_mut().find(|known| known.rule == rule.id) {
                Some(known) => known.nodes.extend(nodes),
                None => violations.push(AxeViolation {
                    rule: rule.id,
                    impact: rule.impact,
                    tags: rule.tags,
                    help: rule.help,
                    help_url: rule.help_url,
                    nodes: nodes.collect(),
                }),
            }
        }
    }
    violations.sort_by(|a, b| a.rule.cmp(&b.rule));
    AxeResults {
        axe_version: version,
        tags: TAGS.iter().map(|tag| (*tag).to_string()).collect(),
        violations,
        incomplete,
        errors: problems.clone(),
    }
}

/// Boxes are rounded to a tenth in the page, so equal boxes can differ by a rounding step.
const SAME_BOX: f64 = 0.51;

fn same_box(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() <= SAME_BOX
        && (a.y - b.y).abs() <= SAME_BOX
        && (a.w - b.w).abs() <= SAME_BOX
        && (a.h - b.h).abs() <= SAME_BOX
}

fn contains_centre(outer: &Rect, inner: &Rect) -> bool {
    let (cx, cy) = (inner.x + inner.w / 2.0, inner.y + inner.h / 2.0);
    cx >= outer.x && cx <= outer.x + outer.w && cy >= outer.y && cy <= outer.y + outer.h
}

/// The component an element belongs to, and whether it was found through containment.
///
/// Only instances in the element's own frame count. An instance with the same box and tag is the
/// element itself. Otherwise the instance with the smallest box that contains the element's
/// centre is its nearest enclosing component (`true`: via an ancestor). `None` when nothing
/// contains it, or when the element has no size (a hidden one): the finding is then about the page.
fn place(frame_id: &str, page_box: &Rect, tag: &str, components: &[Component]) -> Option<(String, bool)> {
    if page_box.w <= 0.0 || page_box.h <= 0.0 {
        return None;
    }
    let instances = || {
        components
            .iter()
            .flat_map(|component| component.instances.iter().map(move |instance| (component, instance)))
            .filter(|(_, instance)| instance.frame_id == frame_id)
    };
    if let Some((component, _)) = instances().find(|(_, i)| i.tag == tag && same_box(&i.box_, page_box)) {
        return Some((component.id.clone(), false));
    }
    let area = |instance: &Instance| instance.box_.w * instance.box_.h;
    instances()
        .filter(|(_, instance)| instance.box_.w > 0.0 && instance.box_.h > 0.0)
        .filter(|(_, instance)| contains_centre(&instance.box_, page_box))
        .min_by(|a, b| area(a.1).total_cmp(&area(b.1)))
        .map(|(component, _)| (component.id.clone(), true))
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // test builders take JSON by value to read like the data they make
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    fn instance(frame: &str, tag: &str, at: [f64; 4]) -> Value {
        json!({
            "key": "k", "name": "n", "tag": tag, "attrs": {}, "selectors": [],
            "box": { "x": at[0], "y": at[1], "w": at[2], "h": at[3] },
            "state": { "visible": true, "enabled": true, "inViewport": true },
            "frameId": frame, "inShadowDom": false
        })
    }

    fn component(id: &str, instances: Vec<Value>) -> Component {
        serde_json::from_value(json!({
            "id": id, "role": "button", "name": "n", "nameIsTemplate": false, "confidence": "high",
            "signals": [], "path": "p", "instances": instances
        }))
        .unwrap()
    }

    #[test]
    fn an_element_with_the_same_box_and_tag_is_the_component_itself() {
        let components = [component(
            "c_a",
            vec![instance("main", "button", [10.0, 20.0, 100.0, 40.0])],
        )];
        // A tenth of rounding does not matter.
        let found = place("main", &rect(10.0, 20.1, 100.0, 40.0), "button", &components);
        assert_eq!(found, Some(("c_a".to_string(), false)));
    }

    #[test]
    fn an_element_inside_a_component_belongs_to_it_through_the_ancestor() {
        let components = [component("c_a", vec![instance("main", "a", [0.0, 0.0, 200.0, 100.0])])];
        let found = place("main", &rect(20.0, 20.0, 50.0, 10.0), "span", &components);
        assert_eq!(found, Some(("c_a".to_string(), true)));
        // The same box with another tag is an element inside, not the component.
        let same = place("main", &rect(0.0, 0.0, 200.0, 100.0), "div", &components);
        assert_eq!(same, Some(("c_a".to_string(), true)));
    }

    #[test]
    fn the_smallest_containing_box_wins() {
        let components = [
            component("c_big", vec![instance("main", "section", [0.0, 0.0, 500.0, 500.0])]),
            component("c_small", vec![instance("main", "a", [10.0, 10.0, 100.0, 50.0])]),
        ];
        let found = place("main", &rect(20.0, 20.0, 30.0, 10.0), "span", &components);
        assert_eq!(found, Some(("c_small".to_string(), true)));
    }

    #[test]
    fn an_element_outside_every_component_is_about_the_page() {
        let components = [component(
            "c_a",
            vec![instance("main", "button", [0.0, 0.0, 50.0, 20.0])],
        )];
        assert_eq!(place("main", &rect(300.0, 300.0, 40.0, 40.0), "img", &components), None);
        // An element without a size (hidden) cannot be placed by its centre.
        assert_eq!(place("main", &rect(10.0, 10.0, 0.0, 0.0), "img", &components), None);
    }

    #[test]
    fn only_components_of_the_same_frame_are_considered() {
        let components = [component(
            "c_a",
            vec![instance("f1", "button", [0.0, 0.0, 400.0, 400.0])],
        )];
        assert_eq!(
            place("main", &rect(10.0, 10.0, 20.0, 20.0), "button", &components),
            None
        );
        assert_eq!(
            place("f1", &rect(10.0, 10.0, 20.0, 20.0), "button", &components),
            Some(("c_a".to_string(), true))
        );
    }

    fn reported(rule: &str, node_box: Rect, tag: &str) -> Reported {
        Reported {
            version: "4.14.0".into(),
            incomplete: 1,
            violations: vec![ReportedRule {
                id: rule.into(),
                impact: Some("serious".into()),
                tags: vec!["wcag2aa".into(), "wcag143".into()],
                help: "help".into(),
                help_url: "https://example.test/help".into(),
                total: 1,
                nodes: vec![ReportedNode {
                    target: "#x".into(),
                    box_: node_box,
                    tag: tag.into(),
                }],
            }],
        }
    }

    #[test]
    fn a_frame_offset_moves_the_element_into_page_coordinates_before_it_is_placed() {
        // The iframe sits at (100, 200). Its button is at (10, 20) inside it, so (110, 220) on the page.
        let components = [component(
            "c_framed",
            vec![instance("f1", "button", [110.0, 220.0, 80.0, 30.0])],
        )];
        let frames = vec![
            (
                "main".to_string(),
                None,
                reported("label", rect(5.0, 5.0, 10.0, 10.0), "input"),
            ),
            (
                "f1".to_string(),
                Some(rect(100.0, 200.0, 300.0, 150.0)),
                reported("color-contrast", rect(10.0, 20.0, 80.0, 30.0), "button"),
            ),
        ];
        let mut problems = Vec::new();
        let merged = assemble(frames, &components, &mut problems);
        assert_eq!(merged.incomplete, 2);
        assert_eq!(merged.axe_version, "4.14.0");
        let contrast = merged.violations.iter().find(|v| v.rule == "color-contrast").unwrap();
        assert_eq!(contrast.nodes[0].component_id.as_deref(), Some("c_framed"));
        assert_eq!(contrast.nodes[0].via_ancestor, None);
        assert_eq!(contrast.nodes[0].box_.x, 110.0);
        let label = merged.violations.iter().find(|v| v.rule == "label").unwrap();
        assert_eq!(label.nodes[0].component_id, None, "main frame has no component");
        assert!(problems.is_empty());
    }

    #[test]
    fn nodes_of_one_rule_in_several_frames_are_joined() {
        let frames = vec![
            (
                "main".to_string(),
                None,
                reported("image-alt", rect(0.0, 0.0, 5.0, 5.0), "img"),
            ),
            (
                "f1".to_string(),
                Some(rect(0.0, 0.0, 9.0, 9.0)),
                reported("image-alt", rect(1.0, 1.0, 5.0, 5.0), "img"),
            ),
        ];
        let merged = assemble(frames, &[], &mut Vec::new());
        assert_eq!(merged.violations.len(), 1);
        assert_eq!(merged.violations[0].nodes.len(), 2);
    }

    #[test]
    fn the_wrapper_is_one_function_around_the_source() {
        let script = AxeScript {
            source: "window.axe = { run: async () => ({}) };".to_string(),
        };
        let function = script.function();
        assert!(function.starts_with("async (tags) => {"));
        assert!(function.contains("window.axe = {"));
        assert!(function.trim_end().ends_with('}'));
        assert!(!function.contains("eval("), "the page's eval is never used");
    }
}
