//! Component-level visual regression from the crops of two runs.
//!
//! Pixels are compared only within one browser and one viewport, and only for components the
//! structural diff found unchanged ([`plan`]). The comparison itself is [`pixels`]. [`visual`]
//! ties them together: it reads the two crops of each comparable component, and writes a diff
//! picture into the output folder for the ones that differ. It never writes into a run folder.
//!
//! Only the report ([`VisualReport`], `spec/visual.schema.json`) and [`render_markdown`] leave
//! this crate; the command line lives in `traceqa-cli`.

pub mod pixels;
pub mod plan;

use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Component as PathPart, Path, PathBuf};

use traceqa_common::types::{
    ComponentsFile, VisualReport, VisualResult, VisualResultStatus, VisualSide, VisualSummary, VisualThresholds,
};
use traceqa_diff::text::code_span;
use traceqa_diff::{compare, Options};

use plan::{Planned, Verdict};

/// `VisualReport.formatVersion`.
pub const FORMAT_VERSION: u64 = 1;
/// Per-pixel colour distance, as in Playwright's `toHaveScreenshot`.
pub const DEFAULT_THRESHOLD: f64 = 0.2;
/// Share of differing pixels a component may have and still be the same.
pub const DEFAULT_MAX_DIFF_RATIO: f64 = 0.001;
/// The largest crop file that is read, in bytes.
const MAX_CROP_BYTES: u64 = 16 * 1024 * 1024;

/// One side: its components, the folder they came from (crop paths are relative to it) and a label.
pub struct Target<'a> {
    pub label: &'a str,
    pub dir: &'a Path,
    /// The target folder name, such as `chromium-1280x800`.
    pub name: Option<&'a str>,
    pub file: &'a ComponentsFile,
}

pub struct Settings {
    pub threshold: f64,
    pub max_diff_ratio: f64,
    /// Where diff pictures go: `<out>/<target>/<id>.diff.png`.
    pub out: PathBuf,
}

/// Why two targets cannot be compared. All of them are bad input (exit code 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualError(pub String);

impl std::fmt::Display for VisualError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VisualError {}

fn fail<T>(message: impl Into<String>) -> Result<T, VisualError> {
    Err(VisualError(message.into()))
}

/// The first number of a browser version (`156` of `156.0.7000.1`), or the whole text when it
/// does not start with one.
fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

fn has_crops(file: &ComponentsFile) -> bool {
    file.components.iter().any(|c| c.crop.is_some())
}

/// Compares the crops of two targets.
pub fn visual(a: &Target, b: &Target, settings: &Settings) -> Result<VisualReport, VisualError> {
    if a.file.browser.name != b.file.browser.name || a.file.viewport != b.file.viewport {
        return fail(format!(
            "pixels are compared only within one browser and viewport; use traceqa diff across browsers \
             (A is {} at {}x{}, B is {} at {}x{})",
            a.file.browser.name,
            a.file.viewport.width,
            a.file.viewport.height,
            b.file.browser.name,
            b.file.viewport.width,
            b.file.viewport.height
        ));
    }
    for (side, target) in [("A", a), ("B", b)] {
        if !has_crops(target.file) {
            return fail(format!(
                "side {side} has no crops; extract it with --crops, then compare again"
            ));
        }
    }
    let environment_match = major(&a.file.browser.version) == major(&b.file.browser.version);
    let diff = compare(a.file, b.file, &Options::default()).map_err(|e| VisualError(e.to_string()))?;
    let folder = b.name.or(a.name).unwrap_or("target");
    let folder = if is_plain_name(folder) { folder } else { "target" };

    let mut results = Vec::new();
    for planned in plan::plan(a.file, b.file, &diff) {
        results.push(result_for(&planned, a, b, settings, folder, environment_match));
    }
    let count = |status: VisualResultStatus| results.iter().filter(|r| r.status == status).count() as u64;
    let summary = VisualSummary {
        compared: count(VisualResultStatus::Same) + count(VisualResultStatus::Different),
        same: count(VisualResultStatus::Same),
        different: count(VisualResultStatus::Different),
        size_changed: count(VisualResultStatus::SizeChanged),
        skipped: count(VisualResultStatus::Skipped),
    };
    Ok(VisualReport {
        format_version: FORMAT_VERSION,
        a: side(a),
        b: side(b),
        thresholds: VisualThresholds {
            threshold: settings.threshold,
            max_diff_ratio: settings.max_diff_ratio,
        },
        environment_match,
        results,
        summary,
    })
}

fn side(target: &Target) -> VisualSide {
    VisualSide {
        label: target.label.to_string(),
        run_id: target.file.run_id.clone(),
        target: target.name.map(str::to_string),
        browser: target.file.browser.clone(),
        viewport: target.file.viewport.clone(),
    }
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':'])
}

fn blank(planned: &Planned, status: VisualResultStatus) -> VisualResult {
    VisualResult {
        id: planned.id.to_string(),
        role: planned.role.to_string(),
        name: planned.name.to_string(),
        status,
        reason: None,
        detail: None,
        unreliable: None,
        diff_pixels: None,
        total_pixels: None,
        diff_ratio: None,
        diff_image: None,
    }
}

fn skipped(planned: &Planned, reason: &str, detail: Option<String>) -> VisualResult {
    VisualResult {
        reason: Some(reason.to_string()),
        detail,
        ..blank(planned, VisualResultStatus::Skipped)
    }
}

fn result_for(
    planned: &Planned,
    a: &Target,
    b: &Target,
    settings: &Settings,
    folder: &str,
    environment_match: bool,
) -> VisualResult {
    let (crop_a, crop_b) = match (&planned.verdict, planned.crops) {
        (Verdict::Skip { reason, detail }, _) => return skipped(planned, reason, detail.clone()),
        (Verdict::SizeChanged(detail), _) => {
            return VisualResult {
                detail: Some(detail.clone()),
                ..blank(planned, VisualResultStatus::SizeChanged)
            }
        }
        (Verdict::Compare, Some(crops)) => crops,
        (Verdict::Compare, None) => return skipped(planned, "no_crop", None),
    };
    let load = |target: &Target, crop: &str| read_crop(target.dir, crop);
    let (image_a, image_b) = match (load(a, crop_a), load(b, crop_b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(Problem::Unsafe(why)), _) | (_, Err(Problem::Unsafe(why))) => {
            return skipped(planned, "unsafe_crop", Some(why))
        }
        (Err(Problem::Unreadable(why)), _) | (_, Err(Problem::Unreadable(why))) => {
            return skipped(planned, "crop_unreadable", Some(why))
        }
    };
    let Some(compared) = pixels::compare(&image_a, &image_b, settings.threshold) else {
        return VisualResult {
            detail: Some(format!(
                "crop {}x{} became {}x{}",
                image_a.width, image_a.height, image_b.width, image_b.height
            )),
            ..blank(planned, VisualResultStatus::SizeChanged)
        };
    };
    let ratio = compared.ratio();
    let same = ratio <= settings.max_diff_ratio;
    let mut result = VisualResult {
        diff_pixels: Some(compared.diff_pixels),
        total_pixels: Some(compared.total_pixels),
        diff_ratio: Some(ratio),
        unreliable: (!environment_match).then_some(true),
        ..blank(
            planned,
            if same {
                VisualResultStatus::Same
            } else {
                VisualResultStatus::Different
            },
        )
    };
    let name = format!("{}.diff.png", planned.id);
    let path = settings.out.join(folder).join(&name);
    if same {
        // A picture from an earlier comparison would say otherwise.
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            let _ = fs::remove_file(&path);
        }
    } else {
        match write_diff(&path, &image_b, &compared) {
            Ok(()) => result.diff_image = Some(format!("{folder}/{name}")),
            Err(why) => result.detail = Some(format!("the diff picture could not be written: {why}")),
        }
    }
    result
}

fn write_diff(path: &Path, image: &pixels::Rgba, compared: &pixels::Comparison) -> Result<(), String> {
    let picture = pixels::diff_image(image, compared).ok_or("the picture has the wrong size")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    picture.save(path).map_err(|e| e.to_string())
}

enum Problem {
    /// The crop path leaves the target folder, or is a link.
    Unsafe(String),
    Unreadable(String),
}

/// Reads and decodes a crop. The path comes from a file that may be hostile, so it must be a plain
/// relative path to a regular file of bounded size inside the target folder.
fn read_crop(dir: &Path, crop: &str) -> Result<pixels::Rgba, Problem> {
    let relative = Path::new(crop);
    if crop.is_empty() || !relative.components().all(|p| matches!(p, PathPart::Normal(_))) {
        return Err(Problem::Unsafe(format!(
            "the crop path {crop:?} is not a plain relative path"
        )));
    }
    let path = dir.join(relative);
    let meta = fs::symlink_metadata(&path).map_err(|e| Problem::Unreadable(format!("{}: {e}", path.display())))?;
    if meta.file_type().is_symlink() {
        return Err(Problem::Unsafe(format!("{} is a symbolic link", path.display())));
    }
    if !meta.is_file() || meta.len() > MAX_CROP_BYTES {
        return Err(Problem::Unreadable(format!(
            "{} is not a regular file of at most {MAX_CROP_BYTES} bytes",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(MAX_CROP_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| Problem::Unreadable(format!("{}: {e}", path.display())))?;
    pixels::decode_png(&bytes).map_err(|e| Problem::Unreadable(format!("{}: {e}", path.display())))
}

/// The most rows shown in each table of the Markdown report.
pub const MAX_ROWS: usize = 50;

/// A Markdown report for a pull request comment. Every string from a page goes through `code_span`.
pub fn render_markdown(report: &VisualReport) -> String {
    let mut out = String::from("## TraceQA visual diff\n\n");
    let describe = |s: &VisualSide| {
        format!(
            "{} ({} {}, {}x{})",
            code_span(&s.label),
            code_span(&s.browser.name),
            code_span(&s.browser.version),
            s.viewport.width,
            s.viewport.height
        )
    };
    let _ = writeln!(out, "- **A**: {}", describe(&report.a));
    let _ = writeln!(out, "- **B**: {}", describe(&report.b));
    let _ = writeln!(
        out,
        "- **Thresholds**: pixel {}, differing share {}\n",
        report.thresholds.threshold, report.thresholds.max_diff_ratio
    );
    if !report.environment_match {
        out.push_str(
            "> **Warning**: the browser versions differ, so pixel differences may come from the browser and not \
             from the page. Treat results as unreliable.\n\n",
        );
    }
    let s = &report.summary;
    let _ = writeln!(
        out,
        "**{} different, {} same, {} resized, {} skipped**\n",
        s.different, s.same, s.size_changed, s.skipped
    );
    let rows = |status: VisualResultStatus| -> Vec<&VisualResult> {
        report.results.iter().filter(|r| r.status == status).collect()
    };
    for (status, title) in [
        (VisualResultStatus::Different, "Different"),
        (VisualResultStatus::SizeChanged, "Resized"),
        (VisualResultStatus::Skipped, "Skipped"),
    ] {
        let rows = rows(status);
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(out, "### {title}\n\n| Component | Detail |\n| --- | --- |");
        for result in rows.iter().take(MAX_ROWS) {
            let detail = match status {
                VisualResultStatus::Different => format!(
                    "{:.2}% of pixels{}",
                    result.diff_ratio.unwrap_or(0.0) * 100.0,
                    result
                        .diff_image
                        .as_ref()
                        .map(|p| format!(", {}", code_span(p)))
                        .unwrap_or_default()
                ),
                VisualResultStatus::SizeChanged => result
                    .detail
                    .clone()
                    .map(|d| traceqa_diff::text::clean(&d))
                    .unwrap_or_default(),
                _ => result.reason.clone().map(|r| code_span(&r)).unwrap_or_default(),
            };
            let _ = writeln!(
                out,
                "| {} {} | {} |",
                code_span(&result.role),
                code_span(&result.name),
                detail
            );
        }
        if rows.len() > MAX_ROWS {
            let _ = writeln!(out, "\n{} more not shown.", rows.len() - MAX_ROWS);
        }
        out.push('\n');
    }
    if report.results.is_empty() {
        out.push_str("No components.\n");
    }
    out
}

#[cfg(test)]
mod tests;
