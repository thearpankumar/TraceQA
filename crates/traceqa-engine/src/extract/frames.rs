//! Reading the page: the collector run in the main page and every frame, and where frames sit.

use serde_json::json;
use traceqa_common::types::{
    CollectorResult, FrameInfo, PageEvalParams, Rect, RequestsFailed, ScrollInfo, SessionOpenResult, Warning,
};

use std::sync::atomic::AtomicBool;

use super::{warning, Job};
use crate::driver::{Driver, DEFAULT_TIMEOUT_MS};
use crate::group::Located;
use crate::url_safety::redact;
use crate::{fingerprint, EngineError, COLLECTOR};

/// What one pass of the collector over the page and its frames produced.
pub(super) struct Reading {
    pub(super) elements: Vec<Located>,
    pub(super) title: String,
    pub(super) scroll: ScrollInfo,
    pub(super) warnings: Vec<Warning>,
    pub(super) frames: usize,
    /// The address of the main page as the browser reports it. Only for reading: never written down.
    pub(super) url: String,
    /// The ids of the frames the driver reached, the main one included.
    pub(super) frame_ids: Vec<String>,
    /// Requests that failed since the page began loading, when the driver reports them. Cumulative,
    /// so the last reading covers the whole load.
    pub(super) requests_failed: Option<RequestsFailed>,
}

/// Runs the collector in the main page and every frame the driver can reach.
pub(super) fn read_frames(driver: &mut Driver, session: &SessionOpenResult, job: &Job) -> Result<Reading, EngineError> {
    read_frames_with(driver, session, job.cancel, job.include_hidden, job.timeout_ms)
}

/// As [`read_frames`], for a caller that has no [`Job`] (the explorer, which reads a page it is already on).
pub(super) fn read_frames_with(
    driver: &mut Driver,
    session: &SessionOpenResult,
    cancel: &AtomicBool,
    include_hidden: bool,
    timeout_ms: Option<u64>,
) -> Result<Reading, EngineError> {
    let evaluated = driver.until(cancel).eval(&PageEvalParams {
        session_id: session.session_id.clone(),
        script: COLLECTOR.to_string(),
        arg: Some(json!({ "includeHidden": include_hidden })),
        all_frames: Some(session.capabilities.eval_all_frames),
        await_promise: None,
        timeout_ms: Some(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
    })?;

    let frames = evaluated.results.len();
    let requests_failed = evaluated.requests_failed;
    let mut elements = Vec::new();
    let mut warnings = Vec::new();
    let mut main: Option<(String, ScrollInfo)> = None;
    let mut url = String::new();
    let mut frame_ids = Vec::new();
    for result in evaluated.results {
        let frame = result.frame;
        let is_main = frame.id == "main";
        frame_ids.push(frame.id.clone());
        if is_main {
            url.clone_from(&frame.url);
        }
        let collected = match result.value {
            Some(value) => serde_json::from_value::<CollectorResult>(value).map_err(|e| e.to_string()),
            None => Err(result.error.unwrap_or_else(|| "the driver gave no reason".to_string())),
        };
        match collected {
            Ok(collected) => {
                warnings.extend(collected.warnings);
                let frame_step = (!is_main).then(|| fingerprint::frame_step_key(frame_label(&frame)));
                for element in collected.elements {
                    let page_box = if is_main {
                        Some(element.box_.clone())
                    } else {
                        frame.box_.as_ref().map(|f| offset(&element.box_, f))
                    };
                    elements.push(Located {
                        element,
                        frame_id: frame.id.clone(),
                        frame_step: frame_step.clone(),
                        page_box,
                        name_changes: false,
                    });
                }
                if is_main {
                    main = Some((collected.title, collected.scroll));
                }
            }
            Err(reason) if is_main => return Err(EngineError::invalid(format!("could not read the page: {reason}"))),
            Err(reason) => {
                let code = if frame.cross_origin {
                    "cross_origin_frame_skipped"
                } else {
                    "frame_eval_failed"
                };
                warnings.push(warning(
                    code,
                    format!("Could not read the frame at {}: {reason}", redact(&frame.url)),
                ));
            }
        }
    }
    let (title, scroll) = main.ok_or_else(|| EngineError::invalid("the driver returned nothing for the main frame"))?;
    Ok(Reading {
        elements,
        title,
        scroll,
        warnings,
        frames,
        url,
        frame_ids,
        requests_failed,
    })
}

/// Identifies one element across readings: its frame and its structural selector.
pub(super) type ElementKey = (String, String);

pub(super) fn element_key(frame_id: &str, selectors: &[String]) -> ElementKey {
    (frame_id.to_string(), selectors.last().cloned().unwrap_or_default())
}

/// Moves a box from a frame's own coordinates to the top-level page.
pub(super) fn offset(inner: &Rect, frame: &Rect) -> Rect {
    Rect {
        x: frame.x + inner.x,
        y: frame.y + inner.y,
        w: inner.w,
        h: inner.h,
    }
}

/// What a frame is called in a component path: its name, or the path of its URL.
pub(super) fn frame_label(frame: &FrameInfo) -> &str {
    if let Some(name) = frame.name.as_deref().filter(|name| !name.is_empty()) {
        return name;
    }
    let after_scheme = frame.url.split_once("://").map_or(frame.url.as_str(), |(_, rest)| rest);
    let path = after_scheme.find('/').map_or("/", |start| &after_scheme[start..]);
    path.split(['?', '#']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(name: Option<&str>, url: &str) -> FrameInfo {
        FrameInfo {
            id: "f1".into(),
            parent_id: None,
            url: url.into(),
            name: name.map(Into::into),
            cross_origin: false,
            box_: None,
        }
    }

    #[test]
    fn frame_labels_are_labelled_by_name_then_by_url_path() {
        assert_eq!(
            frame_label(&frame(Some("pay"), "https://pay.example.com/widget?id=7")),
            "pay"
        );
        assert_eq!(
            frame_label(&frame(None, "https://pay.example.com:8443/widget/v2?id=7#top")),
            "/widget/v2"
        );
        assert_eq!(frame_label(&frame(Some(""), "https://pay.example.com")), "/");
        assert_eq!(frame_label(&frame(None, "about:blank")), "/");
    }

    #[test]
    fn offset_adds_the_frame_origin() {
        let inner = Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };
        let frame = Rect {
            x: 100.0,
            y: 200.0,
            w: 500.0,
            h: 300.0,
        };
        let moved = offset(&inner, &frame);
        assert_eq!((moved.x, moved.y, moved.w, moved.h), (110.0, 220.0, 30.0, 40.0));
    }

    #[test]
    fn elements_are_keyed_by_frame_and_innermost_selector() {
        let selectors = ["#a".to_string(), "body > #a".to_string()];
        assert_eq!(element_key("main", &selectors), ("main".into(), "body > #a".into()));
        assert_eq!(element_key("f1", &[]), ("f1".into(), String::new()));
    }
}
