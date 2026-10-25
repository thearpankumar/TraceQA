//! `traceqa verify --manifest FILE RUN`: check a finished run against a manifest, the list of
//! components a page should have (`spec/manifest.schema.json`, `testbed/sites/buggy-shop/manifest.yaml`).
//!
//! The manifest is a constraint set, not a snapshot: it lists what must be there, what must not be,
//! and which warnings are allowed. Components found that it does not list are extras, which warn
//! (and fail with `--strict`). The matching is the matching of `testbed/bench/run.py`: same role, then a
//! template flag or an exact name, each found component used once, in manifest order. A frame is
//! compared through the label in the component path, never by splitting the path on " > ".
//!
//! Exit 0 when every checked target passes, 1 when one does not, 2 when the manifest or the run
//! cannot be used. Page text in a message goes through `traceqa_diff::text::plain`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use serde_json::{json, Value};
use traceqa_diff::gate::Severity;
use traceqa_diff::github::{self, Annotation};
use traceqa_diff::text::plain;
use traceqa_protocol::types::{
    Component, ComponentsFile, Confidence, ManifestAbsent, ManifestComponent, ManifestFile, RunStatus, VerifyIssue,
    VerifyIssueSeverity, VerifyReport, VerifySkipped, VerifyTarget,
};
use traceqa_recorder::{open_run, read_components};

use crate::manifest;

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    /// GitHub Actions workflow commands; also fills $GITHUB_STEP_SUMMARY.
    Github,
    /// SARIF 2.1.0, for SARIF viewers and other tools.
    Sarif,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// A run folder, a target folder, a components.json, or the id of a run in --runs-dir.
    run: String,
    /// The manifest (YAML) the run must satisfy.
    #[arg(long, value_name = "FILE")]
    manifest: PathBuf,
    /// Only this target, for example chromium-1280x800. Default: all.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
    /// Fail on components the manifest does not list (they only warn otherwise).
    #[arg(long)]
    strict: bool,
    /// How to print.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
    /// Where bare run ids are looked up.
    #[arg(long, value_name = "DIR", default_value = "runs")]
    runs_dir: String,
}

pub fn run(args: &VerifyArgs) -> ExitCode {
    match execute(args) {
        Ok(code) => code,
        Err(why) => {
            eprintln!("traceqa: verify: {why}");
            ExitCode::from(2)
        }
    }
}

fn read_manifest(path: &Path) -> Result<ManifestFile, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?
        .len();
    if size > manifest::MAX_BYTES as u64 {
        return Err(format!("{}: the manifest is too large", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn execute(args: &VerifyArgs) -> Result<ExitCode, String> {
    let expected = read_manifest(&args.manifest)?;
    let run = open_run(&args.run, Path::new(&args.runs_dir)).map_err(|e| e.to_string())?;
    if let Some(wanted) = &args.target {
        if !run.targets.iter().any(|t| &t.dir == wanted) {
            let known: Vec<&str> = run.targets.iter().map(|t| t.dir.as_str()).collect();
            return Err(format!("--target {wanted}: the run has {}", known.join(", ")));
        }
    }
    let mut inputs = Vec::new();
    for target in run
        .targets
        .iter()
        .filter(|t| args.target.as_ref().is_none_or(|w| w == &t.dir))
    {
        if let Some(summary) = target.summary.as_ref().filter(|s| s.status != RunStatus::Completed) {
            return Err(format!(
                "target {} did not finish (status {}); a run with an unfinished target cannot be verified. \
                 Verify the others with --target, or extract again",
                target.dir, summary.status
            ));
        }
        inputs.push((target.dir.clone(), read_components(target).map_err(|e| e.to_string())?));
    }
    let run_id = run.manifest.as_ref().map_or_else(
        || inputs.first().map_or_else(String::new, |(_, f)| f.run_id.clone()),
        |m| m.run_id.clone(),
    );
    let report = verify_run(
        &expected,
        &args.manifest.display().to_string(),
        &run_id,
        &inputs,
        args.strict,
    );
    if report.targets.is_empty() {
        return Err(format!(
            "no target of the run has a viewport the manifest names ({}); nothing was checked",
            report
                .skipped
                .iter()
                .map(|s| s.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    match args.format {
        Format::Text => print!("{}", render_text(&report)),
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("spec types always serialise")
        ),
        Format::Github => {
            print!("{}", github::render(&annotations(&report)));
            append_step_summary(&render_text(&report));
        }
        Format::Sarif => println!(
            "{}",
            serde_json::to_string_pretty(&sarif(&report)).expect("JSON always serialises")
        ),
    }
    Ok(if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Checks every target whose viewport the manifest names; the others are listed as skipped.
fn verify_run(
    expected: &ManifestFile,
    manifest_name: &str,
    run_id: &str,
    inputs: &[(String, ComponentsFile)],
    strict: bool,
) -> VerifyReport {
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    let mut seen_warnings = BTreeSet::new();
    for (dir, file) in inputs {
        let Some(viewport) = manifest::viewport_name_for(expected, &file.viewport) else {
            let mut known: Vec<String> = expected
                .viewports
                .iter()
                .map(|(n, v)| format!("{n} {}x{}", v.width, v.height))
                .collect();
            known.sort();
            skipped.push(VerifySkipped {
                target: dir.clone(),
                reason: format!(
                    "{dir}: viewport {}x{} is not in the manifest ({})",
                    file.viewport.width,
                    file.viewport.height,
                    known.join(", ")
                ),
            });
            continue;
        };
        seen_warnings.extend(file.warnings.iter().map(|w| w.code.clone()));
        let issues = check_target(expected, &viewport, file, strict);
        targets.push(VerifyTarget {
            target: dir.clone(),
            url: file.url.clone(),
            viewport,
            size: file.viewport.clone(),
            checked: expected.components.len() as u64,
            passed: !issues.iter().any(|i| i.severity == VerifyIssueSeverity::Fail),
            issues,
        });
    }
    let mut notes = Vec::new();
    if !targets.is_empty() {
        for code in &expected.warnings {
            if !seen_warnings.contains(code) {
                notes.push(format!(
                    "stale: warning {} is allowed by the manifest but no checked target raised it; remove it if it is no longer needed",
                    quoted(code)
                ));
            }
        }
    }
    VerifyReport {
        format_version: std::num::NonZeroU64::MIN,
        run_id: run_id.to_string(),
        manifest: manifest_name.to_string(),
        strict,
        passed: targets.iter().all(|t| t.passed),
        targets,
        skipped,
        notes,
    }
}

fn quoted(text: &str) -> String {
    format!("'{}'", plain(text))
}

/// `button 'Open size guide'`, or `link (template)` for a component whose names differ.
fn subject(role: &str, name: &str, template: bool) -> String {
    if template {
        format!("{} (template)", plain(role))
    } else {
        format!("{} {}", plain(role), quoted(name))
    }
}

fn expected_subject(c: &ManifestComponent) -> String {
    subject(&c.role, c.name.as_deref().unwrap_or(""), c.template == Some(true))
}

fn found_subject(c: &Component) -> String {
    subject(&c.role, &c.name, c.name_is_template)
}

/// The rule `testbed/bench/run.py` calls `matches`: the role, then the template flag or the exact name.
fn matches(expected: &ManifestComponent, found: &Component) -> bool {
    if found.role != expected.role {
        return false;
    }
    if expected.template == Some(true) {
        return found.name_is_template;
    }
    !found.name_is_template && found.name == expected.name.as_deref().unwrap_or("")
}

fn matches_absent(absent: &ManifestAbsent, found: &Component) -> bool {
    found.role == absent.role && !found.name_is_template && found.name == absent.name
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != cb)).min(row[j] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The unmatched component of the same role whose name is closest, when it is close enough to be a
/// likely rename or typo.
fn nearest<'a>(expected: &ManifestComponent, candidates: &[&'a Component]) -> Option<&'a Component> {
    if expected.template == Some(true) {
        return None;
    }
    let want = expected.name.as_deref().unwrap_or("");
    candidates
        .iter()
        .filter(|c| c.role == expected.role && !c.name_is_template)
        .map(|c| (edit_distance(want, &c.name), *c))
        .filter(|(d, c)| *d <= 2 || d * 2 <= want.chars().count().max(c.name.chars().count()))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Low => "low",
    }
}

fn issue(severity: VerifyIssueSeverity, check: &str, message: String) -> VerifyIssue {
    VerifyIssue {
        severity,
        check: check.to_string(),
        message,
        role: None,
        name: None,
        component_id: None,
    }
}

/// Everything wrong with one target at one manifest viewport, in manifest order: components, then
/// absent entries, then warnings, then extras.
fn check_target(expected: &ManifestFile, viewport: &str, file: &ComponentsFile, strict: bool) -> Vec<VerifyIssue> {
    let mut issues = Vec::new();
    let mut taken = vec![false; file.components.len()];
    let mut matched: Vec<Option<usize>> = Vec::new();
    for want in &expected.components {
        let found = file
            .components
            .iter()
            .enumerate()
            .position(|(i, c)| !taken[i] && matches(want, c));
        if let Some(i) = found {
            taken[i] = true;
        }
        matched.push(found);
    }
    let leftovers: Vec<&Component> = file
        .components
        .iter()
        .enumerate()
        .filter(|(i, _)| !taken[*i])
        .map(|(_, c)| c)
        .collect();

    for (want, found) in expected.components.iter().zip(&matched) {
        let what = expected_subject(want);
        let Some(found) = found.map(|i| &file.components[i]) else {
            let mut message = format!("{what} x{}", want.instances.unwrap_or(1));
            if let Some(near) = nearest(want, &leftovers) {
                let _ = write!(message, " (nearest: {} {})", found_subject(near), near.id);
            }
            issues.push(issue(VerifyIssueSeverity::Fail, "missing", message));
            continue;
        };
        let mut fail = |check: &str, text: String| {
            let mut i = issue(VerifyIssueSeverity::Fail, check, format!("{what}: {text}"));
            i.role = Some(plain(&found.role));
            i.name = Some(plain(&found.name));
            i.component_id = Some(found.id.clone());
            issues.push(i);
        };
        let count = found.instances.len() as u64;
        if want.instances.unwrap_or(1) != count {
            fail(
                "instances",
                format!("expected {} instances, found {count}", want.instances.unwrap_or(1)),
            );
        }
        let want_confidence = want.confidence.unwrap_or(Confidence::High);
        if want_confidence != found.confidence {
            fail(
                "confidence",
                format!(
                    "expected confidence {}, found {}",
                    confidence_name(want_confidence),
                    confidence_name(found.confidence)
                ),
            );
        }
        let visible = found.instances.iter().filter(|i| i.state.visible).count();
        if want.hidden.iter().any(|h| h == viewport) {
            if visible > 0 {
                fail(
                    "hidden",
                    format!("listed as hidden at {viewport} but {visible} of {count} instances are visible"),
                );
            }
        } else if visible == 0 {
            fail(
                "visible",
                format!("not listed as hidden at {viewport} but none of its {count} instances is visible"),
            );
        }
        let disabled = found.instances.iter().filter(|i| !i.state.enabled).count() as u64;
        if want.disabled.unwrap_or(0) != disabled {
            fail(
                "disabled",
                format!("expected {} disabled, found {disabled}", want.disabled.unwrap_or(0)),
            );
        }
        let in_shadow = found.instances.iter().all(|i| i.in_shadow_dom);
        if want.shadow == Some(true) && !in_shadow {
            fail("shadow", "expected inside shadow DOM, found outside it".into());
        } else if want.shadow != Some(true) && in_shadow {
            fail("shadow", "expected outside shadow DOM, found inside it".into());
        }
        let want_frame = want.frame.as_deref().map(manifest::normalise_frame);
        let got_frame = manifest::frame_of(&found.path);
        if want_frame != got_frame {
            let describe = |f: &Option<String>| {
                f.as_ref()
                    .map_or("the main page".to_string(), |f| format!("frame {}", quoted(f)))
            };
            fail(
                "frame",
                format!("expected {}, found {}", describe(&want_frame), describe(&got_frame)),
            );
        }
    }

    for absent in &expected.absent {
        for found in file.components.iter().filter(|c| matches_absent(absent, c)) {
            let mut i = issue(
                VerifyIssueSeverity::Fail,
                "present",
                format!(
                    "{} must not exist but was found ({}); it should only appear after a click",
                    subject(&absent.role, &absent.name, false),
                    found.id
                ),
            );
            i.role = Some(plain(&found.role));
            i.name = Some(plain(&found.name));
            i.component_id = Some(found.id.clone());
            issues.push(i);
        }
    }

    let observed: BTreeSet<&str> = file.warnings.iter().map(|w| w.code.as_str()).collect();
    for code in observed {
        if !expected.warnings.iter().any(|w| w == code) {
            let detail = file
                .warnings
                .iter()
                .find(|w| w.code == code)
                .map(|w| plain(&w.message))
                .unwrap_or_default();
            issues.push(issue(
                VerifyIssueSeverity::Fail,
                "warning",
                format!(
                    "unexpected warning {} ({detail}); allow it under warnings in the manifest or fix the page",
                    quoted(code)
                ),
            ));
        }
    }

    for extra in leftovers {
        let mut i = issue(
            if strict {
                VerifyIssueSeverity::Fail
            } else {
                VerifyIssueSeverity::Warning
            },
            "extra",
            format!(
                "{} x{} {} is on the page but not in the manifest",
                found_subject(extra),
                extra.instances.len(),
                extra.id
            ),
        );
        i.role = Some(plain(&extra.role));
        i.name = Some(plain(&extra.name));
        i.component_id = Some(extra.id.clone());
        issues.push(i);
    }
    issues
}

fn label(severity: VerifyIssueSeverity) -> &'static str {
    match severity {
        VerifyIssueSeverity::Fail => "FAIL",
        VerifyIssueSeverity::Warning => "WARN",
        VerifyIssueSeverity::Note => "NOTE",
    }
}

fn render_text(report: &VerifyReport) -> String {
    let mut out = format!(
        "Verifying run {} against {}{}\n",
        plain(&report.run_id),
        plain(&report.manifest),
        if report.strict { " (strict)" } else { "" }
    );
    for target in &report.targets {
        let fails = target
            .issues
            .iter()
            .filter(|i| i.severity == VerifyIssueSeverity::Fail)
            .count();
        let warns = target
            .issues
            .iter()
            .filter(|i| i.severity == VerifyIssueSeverity::Warning)
            .count();
        let _ = writeln!(
            out,
            "{} ({}): {}, {} components checked{}{}",
            plain(&target.target),
            plain(&target.viewport),
            if target.passed { "PASS" } else { "FAIL" },
            target.checked,
            if fails > 0 {
                format!(", {fails} failures")
            } else {
                String::new()
            },
            if warns > 0 {
                format!(", {warns} warnings")
            } else {
                String::new()
            },
        );
        for i in &target.issues {
            let _ = writeln!(
                out,
                "  {} {} at {}: {}",
                label(i.severity),
                i.check,
                plain(&target.viewport),
                i.message
            );
        }
    }
    for s in &report.skipped {
        let _ = writeln!(out, "skipped {}", s.reason);
    }
    for note in &report.notes {
        let _ = writeln!(out, "NOTE {note}");
    }
    out.push_str(if report.passed { "\nPASS\n" } else { "\nFAIL\n" });
    out
}

fn annotations(report: &VerifyReport) -> Vec<Annotation> {
    let mut items = Vec::new();
    for target in &report.targets {
        for i in &target.issues {
            items.push(Annotation {
                severity: match i.severity {
                    VerifyIssueSeverity::Fail => Severity::Error,
                    VerifyIssueSeverity::Warning => Severity::Warning,
                    VerifyIssueSeverity::Note => Severity::Note,
                },
                title: format!("verify {}", i.check),
                message: format!("{} ({}): {}", plain(&target.target), plain(&target.viewport), i.message),
            });
        }
    }
    for note in &report.notes {
        items.push(Annotation {
            severity: Severity::Note,
            title: "verify stale".to_string(),
            message: note.clone(),
        });
    }
    items
}

/// GitHub Actions names a file whose Markdown becomes the step's summary. The report goes in a
/// fenced block with its backticks removed, so page text cannot end the fence.
fn append_step_summary(text: &str) {
    use std::io::Write as _;
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|p| !p.is_empty()) else {
        return;
    };
    let body = format!("## TraceQA verify\n\n```text\n{}```\n", text.replace('`', "'"));
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(body.as_bytes()));
    if let Err(e) = written {
        eprintln!("traceqa: warning: could not write $GITHUB_STEP_SUMMARY: {e}");
    }
}

/// The SARIF 2.1.0 log of a report. A page has no source file, so a result's location is the page
/// address with a logical location for the target and component, as in the other SARIF output.
fn sarif(report: &VerifyReport) -> Value {
    const CHECKS: [(&str, &str); 11] = [
        ("missing", "A component the manifest lists was not found"),
        ("present", "A component the manifest says must not exist was found"),
        (
            "instances",
            "A component has a different number of instances than the manifest says",
        ),
        (
            "confidence",
            "A component has a different confidence than the manifest says",
        ),
        ("visible", "A component is not visible where the manifest says it is"),
        ("hidden", "A component is visible where the manifest says it is hidden"),
        (
            "disabled",
            "A component has a different number of disabled instances than the manifest says",
        ),
        ("frame", "A component is in another frame than the manifest says"),
        ("shadow", "A component is in or out of shadow DOM against the manifest"),
        ("warning", "The extraction raised a warning the manifest does not allow"),
        ("extra", "A component is on the page but not in the manifest"),
    ];
    let level = |s: VerifyIssueSeverity| match s {
        VerifyIssueSeverity::Fail => "error",
        VerifyIssueSeverity::Warning => "warning",
        VerifyIssueSeverity::Note => "note",
    };
    let rules: Vec<Value> = CHECKS
        .iter()
        .map(|(id, text)| {
            json!({
                "id": format!("traceqa.verify.{id}"),
                "shortDescription": { "text": text },
                "fullDescription": { "text": format!("{text}. The manifest is the file named with --manifest.") },
                "help": { "text": "Fix the page, or edit the manifest if the change is intended. See docs/usage.md (traceqa verify)." },
            })
        })
        .collect();
    let mut results = Vec::new();
    for target in &report.targets {
        for i in &target.issues {
            let Some(index) = CHECKS.iter().position(|(id, _)| *id == i.check) else {
                continue;
            };
            let component = i.component_id.clone().unwrap_or_default();
            results.push(json!({
                "ruleId": format!("traceqa.verify.{}", i.check),
                "ruleIndex": index,
                "level": level(i.severity),
                "message": { "text": format!("{} ({}): {}", plain(&target.target), plain(&target.viewport), i.message) },
                "locations": [{
                    "physicalLocation": { "artifactLocation": { "uri": target.url } },
                    "logicalLocations": [{
                        "name": if component.is_empty() { target.target.clone() } else { component.clone() },
                        "fullyQualifiedName": format!("{}#{}/{}", target.url, target.target, component),
                        "kind": "element",
                    }],
                }],
                "partialFingerprints": {
                    "traceqaVerify/v1": format!("{}/{}/{}/{}", target.target, i.check, i.role.clone().unwrap_or_default(), i.name.clone().unwrap_or_default()),
                },
            }));
        }
    }
    json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "traceqa",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": env!("CARGO_PKG_REPOSITORY"),
                "rules": rules,
            }},
            "results": results,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = include_str!("../../../spec/examples/ComponentsFile.shop.json");
    const HEAD: &str = "viewports:\n  desktop: { width: 1280, height: 800 }\n  mobile: { width: 390, height: 844 }\n";

    /// A components file whose components are described as (role, name, instances), all visible
    /// and enabled, in the main page, with no warnings.
    fn file(components: &[(&str, &str, usize)]) -> ComponentsFile {
        let mut doc: Value = serde_json::from_str(SHOP).unwrap();
        let template = doc["components"][0].clone();
        let list: Vec<Value> = components
            .iter()
            .enumerate()
            .map(|(n, (role, name, count))| {
                let mut c = template.clone();
                c["id"] = json!(format!("c_{n:012x}"));
                c["role"] = json!(role);
                c["name"] = json!(name);
                c["path"] = json!(format!("main > item {n}"));
                let instance = template["instances"][0].clone();
                c["instances"] = Value::Array((0..*count).map(|_| instance.clone()).collect());
                c
            })
            .collect();
        doc["components"] = Value::Array(list);
        doc["warnings"] = json!([]);
        serde_json::from_value(doc).unwrap()
    }

    fn check(manifest_body: &str, found: &ComponentsFile, viewport: &str, strict: bool) -> Vec<String> {
        let m = manifest::parse(&format!("{HEAD}{manifest_body}")).unwrap();
        check_target(&m, viewport, found, strict)
            .iter()
            .map(|i| format!("{} {}: {}", label(i.severity), i.check, i.message))
            .collect()
    }

    #[test]
    fn a_matching_page_passes() {
        let found = file(&[("button", "Go", 1), ("link", "Home", 2)]);
        let out = check(
            "components:\n  - { role: button, name: Go }\n  - { role: link, name: Home, instances: 2 }\n",
            &found,
            "desktop",
            true,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_missing_component_names_the_nearest_one() {
        let found = file(&[("button", "Open the size guide", 1), ("link", "Open size guide", 1)]);
        let out = check(
            "components:\n  - { role: button, name: Open size guide }\n",
            &found,
            "desktop",
            false,
        );
        assert_eq!(
            out[0],
            "FAIL missing: button 'Open size guide' x1 (nearest: button 'Open the size guide' c_000000000000)"
        );
        // The link is the same words but another role, so it is not offered.
        assert!(
            out[1].starts_with("WARN extra: button 'Open the size guide' x1"),
            "{out:?}"
        );
    }

    #[test]
    fn a_missing_component_with_nothing_close_has_no_hint() {
        let found = file(&[("button", "Zebra", 1)]);
        let out = check(
            "components:\n  - { role: button, name: Open size guide, instances: 3 }\n",
            &found,
            "desktop",
            false,
        );
        assert_eq!(out[0], "FAIL missing: button 'Open size guide' x3");
    }

    #[test]
    fn an_absent_component_that_exists_fails() {
        let found = file(&[("button", "Close size guide", 1)]);
        let out = check(
            "absent:\n  - { role: button, name: Close size guide }\ncomponents: []\n",
            &found,
            "desktop",
            false,
        );
        assert!(
            out[0].starts_with("FAIL present: button 'Close size guide' must not exist but was found (c_000000000000)"),
            "{out:?}"
        );
    }

    #[test]
    fn counts_confidence_and_disabled_are_compared() {
        let mut found = file(&[("button", "Go", 2)]);
        found.components[0].confidence = Confidence::Low;
        found.components[0].instances[0].state.enabled = false;
        let out = check(
            "components:\n  - { role: button, name: Go, instances: 3, disabled: 0 }\n",
            &found,
            "desktop",
            false,
        );
        assert_eq!(
            out,
            [
                "FAIL instances: button 'Go': expected 3 instances, found 2",
                "FAIL confidence: button 'Go': expected confidence high, found low",
                "FAIL disabled: button 'Go': expected 0 disabled, found 1",
            ]
        );
    }

    #[test]
    fn hidden_is_checked_only_at_the_viewport_it_names() {
        let mut found = file(&[("link", "Home", 2)]);
        for i in &mut found.components[0].instances {
            i.state.visible = false;
        }
        let body = "components:\n  - { role: link, name: Home, instances: 2, hidden: [mobile] }\n";
        assert_eq!(
            check(body, &found, "desktop", false),
            ["FAIL visible: link 'Home': not listed as hidden at desktop but none of its 2 instances is visible"]
        );
        assert!(check(body, &found, "mobile", false).is_empty());
        found.components[0].instances[1].state.visible = true;
        assert_eq!(
            check(body, &found, "mobile", false),
            ["FAIL hidden: link 'Home': listed as hidden at mobile but 1 of 2 instances are visible"]
        );
        assert!(check(body, &found, "desktop", false).is_empty());
    }

    #[test]
    fn an_unlisted_warning_fails_and_an_unobserved_allowed_one_is_only_a_note() {
        let mut found = file(&[]);
        found.warnings = serde_json::from_value(json!([{ "code": "slow_page", "message": "took 9 s" }])).unwrap();
        let out = check("components: []\n", &found, "desktop", false);
        assert!(
            out[0].starts_with("FAIL warning: unexpected warning 'slow_page' (took 9 s)"),
            "{out:?}"
        );
        let m = manifest::parse(&format!("{HEAD}warnings: [slow_page, old_code]\ncomponents: []\n")).unwrap();
        let report = verify_run(&m, "m.yaml", "r", &[("t".into(), found)], false);
        assert!(report.passed && report.targets[0].issues.is_empty());
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].contains("'old_code'"));
    }

    #[test]
    fn a_template_component_matches_any_names_once() {
        let mut found = file(&[("link", "Product 1", 12), ("link", "Home", 1)]);
        found.components[0].name_is_template = true;
        let out = check(
            "components:\n  - { role: link, template: true, instances: 12 }\n  - { role: link, name: Home }\n",
            &found,
            "desktop",
            true,
        );
        assert!(out.is_empty(), "{out:?}");
        let missing = check(
            "components:\n  - { role: link, template: true }\n",
            &file(&[("link", "Home", 1)]),
            "desktop",
            true,
        );
        assert_eq!(missing[0], "FAIL missing: link (template) x1");
    }

    #[test]
    fn frame_and_shadow_flags_are_compared() {
        let mut found = file(&[("textbox", "Email", 1), ("button", "Plus", 1)]);
        found.components[0].path = "frame \"news \\\" > letter\" > form".into();
        for i in &mut found.components[1].instances {
            i.in_shadow_dom = true;
        }
        let good = "components:\n  - { role: textbox, name: Email, frame: 'News  \" > Letter' }\n  - { role: button, name: Plus, shadow: true }\n";
        assert!(check(good, &found, "desktop", true).is_empty());
        let bad = "components:\n  - { role: textbox, name: Email }\n  - { role: button, name: Plus }\n";
        let out = check(bad, &found, "desktop", true);
        assert_eq!(
            out[0],
            "FAIL frame: textbox 'Email': expected the main page, found frame 'news \" > letter'"
        );
        assert_eq!(
            out[1],
            "FAIL shadow: button 'Plus': expected outside shadow DOM, found inside it"
        );
        let other = "components:\n  - { role: textbox, name: Email, frame: pay }\n  - { role: button, name: Plus, shadow: true }\n";
        assert!(check(other, &found, "desktop", true)[0].contains("expected frame 'pay'"));
    }

    #[test]
    fn extras_warn_and_with_strict_fail() {
        let found = file(&[("button", "Go", 1), ("button", "Stop", 1)]);
        let body = "components:\n  - { role: button, name: Go }\n";
        assert_eq!(
            check(body, &found, "desktop", false),
            ["WARN extra: button 'Stop' x1 c_000000000001 is on the page but not in the manifest"]
        );
        assert!(check(body, &found, "desktop", true)[0].starts_with("FAIL extra:"));
    }

    #[test]
    fn page_text_in_a_message_is_cleaned() {
        let found = file(&[("button", "Go\n::error::\u{202e}pwned", 1)]);
        let out = check("components: []\n", &found, "desktop", false);
        assert!(!out[0].contains('\n') && !out[0].contains('\u{202e}'), "{out:?}");
    }

    #[test]
    fn a_viewport_the_manifest_does_not_name_is_skipped() {
        let mut found = file(&[]);
        found.viewport.width = 800;
        let m = manifest::parse(&format!("{HEAD}components: []\n")).unwrap();
        let report = verify_run(&m, "m.yaml", "r", &[("t".into(), found)], false);
        assert!(report.targets.is_empty() && report.skipped.len() == 1);
        assert!(report.skipped[0].reason.contains("800x800"), "{:?}", report.skipped);
    }

    #[test]
    fn edit_distance_is_the_levenshtein_distance() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("Go", "go"), 0);
    }
}
