//! End to end on small generated PNG crops in a temporary folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use traceqa_protocol::types::{ComponentsFile, VisualResultStatus};

use super::*;

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "traceqa-visual-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp folder");
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn component(id: &str, crop: Option<&str>) -> Value {
    let mut value = json!({
        "id": id, "role": "button", "name": id, "nameIsTemplate": false, "confidence": "high",
        "signals": [], "path": "main",
        "instances": [{
            "key": format!("{id}:0"), "name": id, "tag": "button", "attrs": {}, "selectors": ["button"],
            "box": {"x": 0, "y": 0, "w": 20, "h": 10},
            "state": {"visible": true, "enabled": true, "inViewport": true},
            "frameId": "main", "inShadowDom": false
        }]
    });
    if let Some(crop) = crop {
        value["crop"] = json!(crop);
    }
    value
}

#[allow(clippy::needless_pass_by_value)]
fn file(browser: &str, version: &str, components: Vec<Value>) -> ComponentsFile {
    serde_json::from_value(json!({
        "formatVersion": 1, "fingerprintVersion": 1, "runId": "r", "url": "http://x/", "finalUrl": "http://x/",
        "title": "t", "browser": {"name": browser, "version": version},
        "viewport": {"width": 1280, "height": 800}, "capturedAt": "2026-01-01T00:00:00Z",
        "components": components, "warnings": []
    }))
    .expect("a valid file")
}

fn save(dir: &Path, name: &str, width: u32, height: u32, dots: &[(u32, u32)]) {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    for (x, y) in dots {
        image.put_pixel(*x, *y, Rgba([0, 0, 0, 255]));
    }
    fs::create_dir_all(dir.join("crops")).expect("crops folder");
    image.save(dir.join("crops").join(name)).expect("png");
}

struct Fixture {
    temp: Temp,
    a: PathBuf,
    b: PathBuf,
}

fn fixture() -> Fixture {
    let temp = Temp::new();
    let (a, b) = (temp.0.join("a"), temp.0.join("b"));
    Fixture { temp, a, b }
}

fn run(
    f: &Fixture,
    a: &ComponentsFile,
    b: &ComponentsFile,
    ratio: f64,
) -> Result<traceqa_protocol::types::VisualReport, VisualError> {
    visual(
        &Target {
            label: "A",
            dir: &f.a,
            name: Some("chromium-1280x800"),
            file: a,
        },
        &Target {
            label: "B",
            dir: &f.b,
            name: Some("chromium-1280x800"),
            file: b,
        },
        &Settings {
            threshold: DEFAULT_THRESHOLD,
            max_diff_ratio: ratio,
            out: f.temp.0.join("out"),
        },
    )
}

fn status<'a>(
    report: &'a traceqa_protocol::types::VisualReport,
    id: &str,
) -> &'a traceqa_protocol::types::VisualResult {
    report.results.iter().find(|r| r.id == id).expect("a result")
}

#[test]
fn same_and_different_components_with_a_diff_picture_for_the_different_one() {
    let f = fixture();
    save(&f.a, "c_1.png", 20, 10, &[]);
    save(&f.b, "c_1.png", 20, 10, &[]);
    save(&f.a, "c_2.png", 20, 10, &[]);
    let dots: Vec<(u32, u32)> = (0..10).map(|x| (x, 3)).collect();
    save(&f.b, "c_2.png", 20, 10, &dots);
    let components = || {
        vec![
            component("c_1", Some("crops/c_1.png")),
            component("c_2", Some("crops/c_2.png")),
        ]
    };
    let (a, b) = (
        file("chromium", "156.0.1", components()),
        file("chromium", "156.0.2", components()),
    );
    let report = run(&f, &a, &b, 0.001).expect("compares");
    assert_eq!(status(&report, "c_1").status, VisualResultStatus::Same);
    let changed = status(&report, "c_2");
    assert_eq!(changed.status, VisualResultStatus::Different);
    assert_eq!(changed.diff_pixels, Some(10));
    assert_eq!(changed.diff_image.as_deref(), Some("chromium-1280x800/c_2.diff.png"));
    assert!(f
        .temp
        .0
        .join("out")
        .join("chromium-1280x800")
        .join("c_2.diff.png")
        .is_file());
    assert!(!f
        .temp
        .0
        .join("out")
        .join("chromium-1280x800")
        .join("c_1.diff.png")
        .exists());
    assert!(report.environment_match, "same major version");
    assert_eq!(
        (report.summary.compared, report.summary.same, report.summary.different),
        (2, 1, 1)
    );
    // A looser ratio calls the same pictures the same.
    let loose = run(&f, &a, &b, 0.1).expect("compares");
    assert_eq!(status(&loose, "c_2").status, VisualResultStatus::Same);
}

#[test]
fn another_major_version_marks_compared_results_unreliable() {
    let f = fixture();
    save(&f.a, "c_1.png", 20, 10, &[]);
    save(&f.b, "c_1.png", 20, 10, &[]);
    let one = || vec![component("c_1", Some("crops/c_1.png"))];
    let report = run(
        &f,
        &file("chromium", "155.0.1", one()),
        &file("chromium", "156.0.1", one()),
        0.001,
    )
    .expect("ok");
    assert!(!report.environment_match);
    assert_eq!(status(&report, "c_1").unreliable, Some(true));
}

#[test]
fn another_browser_or_viewport_is_refused() {
    let f = fixture();
    let one = || vec![component("c_1", Some("crops/c_1.png"))];
    let error = run(&f, &file("chromium", "1", one()), &file("firefox", "1", one()), 0.001).expect_err("refused");
    assert!(error.0.contains("only within one browser and viewport"), "{error}");
    let mut other = file("chromium", "1", one());
    other.viewport.width = 800;
    assert!(run(&f, &file("chromium", "1", one()), &other, 0.001).is_err());
}

#[test]
fn a_side_without_crops_is_refused_with_the_hint() {
    let f = fixture();
    let (a, b) = (
        file("chromium", "1", vec![component("c_1", Some("crops/c_1.png"))]),
        file("chromium", "1", vec![component("c_1", None)]),
    );
    let error = run(&f, &a, &b, 0.001).expect_err("refused");
    assert!(error.0.contains("side B") && error.0.contains("--crops"), "{error}");
}

#[test]
fn crops_of_another_size_are_size_changed() {
    let f = fixture();
    save(&f.a, "c_1.png", 20, 10, &[]);
    save(&f.b, "c_1.png", 40, 20, &[]);
    let one = || vec![component("c_1", Some("crops/c_1.png"))];
    let report = run(&f, &file("chromium", "1", one()), &file("chromium", "1", one()), 0.001).expect("ok");
    let result = status(&report, "c_1");
    assert_eq!(result.status, VisualResultStatus::SizeChanged);
    assert_eq!(result.detail.as_deref(), Some("crop 20x10 became 40x20"));
    assert_eq!(report.summary.size_changed, 1);
}

#[test]
fn a_crop_path_that_leaves_the_folder_or_is_missing_is_skipped_not_read() {
    let f = fixture();
    fs::create_dir_all(&f.a).expect("folder");
    fs::write(f.temp.0.join("secret.png"), b"x").expect("file");
    save(&f.a, "c_2.png", 20, 10, &[]);
    save(&f.b, "c_2.png", 20, 10, &[]);
    let list = |escape: &str| {
        vec![
            component("c_1", Some(escape)),
            component("c_2", Some("crops/c_2.png")),
            component("c_3", Some("crops/missing.png")),
        ]
    };
    let report = run(
        &f,
        &file("chromium", "1", list("../secret.png")),
        &file("chromium", "1", list("../secret.png")),
        0.001,
    )
    .expect("ok");
    assert_eq!(status(&report, "c_1").reason.as_deref(), Some("unsafe_crop"));
    assert_eq!(status(&report, "c_2").status, VisualResultStatus::Same);
    assert_eq!(status(&report, "c_3").reason.as_deref(), Some("crop_unreadable"));
}

#[test]
fn a_component_that_is_covered_is_skipped_even_with_equal_crops() {
    let f = fixture();
    save(&f.a, "c_1.png", 20, 10, &[]);
    save(&f.b, "c_1.png", 20, 10, &[(1, 1)]);
    let mut covered = component("c_1", Some("crops/c_1.png"));
    covered["instances"][0]["state"]["covered"] = json!(true);
    let a = file("chromium", "1", vec![covered.clone()]);
    let b = file("chromium", "1", vec![covered]);
    let report = run(&f, &a, &b, 0.0).expect("ok");
    assert_eq!(status(&report, "c_1").reason.as_deref(), Some("covered"));
    assert_eq!(report.summary.compared, 0);
}

#[test]
fn the_markdown_names_the_different_and_the_skipped() {
    let f = fixture();
    save(&f.a, "c_1.png", 20, 10, &[]);
    save(&f.b, "c_1.png", 20, 10, &[(1, 1), (5, 5), (9, 9)]);
    let one = || vec![component("c_1", Some("crops/c_1.png")), component("c_2", None)];
    let report = run(&f, &file("chromium", "1", one()), &file("chromium", "1", one()), 0.0).expect("ok");
    let text = render_markdown(&report);
    assert!(
        text.contains("1 different") && text.contains("### Different") && text.contains("### Skipped"),
        "{text}"
    );
    assert!(text.contains("`no_crop`"), "{text}");
}
