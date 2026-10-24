//! Going through the page: scrolling screen by screen, with or without pictures.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

use traceqa_protocol::types::{
    PageEvalParams, PageScrollParams, Rect, ScrollInfo, ScrollTo, ScrollWait, SessionOpenResult, Warning,
};

use super::crops::screenshot;
use super::frames::{element_key, read_frames, ElementKey, Reading};
use super::heuristics::Removals;
use super::progress::{timed, Clock};
use super::{check_cancelled, Job};
use crate::driver::Driver;
use crate::EngineError;

/// One screenful of the page as it looked while the engine scrolled through it.
pub(super) struct Screen {
    pub(super) image: PathBuf,
    /// Page y coordinate of the top edge of the screen.
    pub(super) top: f64,
    pub(super) height: f64,
    /// Where each visible element was at that moment, in page coordinates. Elements in pinned
    /// or animated sections move as the page scrolls, so this is recorded per screen.
    pub(super) shown: HashMap<ElementKey, Rect>,
}

impl Screen {
    /// How much of `area` this screen shows, from 0 to 1.
    pub(super) fn coverage(&self, area: &Rect, width: f64) -> f64 {
        let across = (area.x + area.w).min(width) - area.x.max(0.0);
        let down = (area.y + area.h).min(self.top + self.height) - area.y.max(self.top);
        if area.w <= 0.0 || area.h <= 0.0 || across <= 0.0 || down <= 0.0 {
            return 0.0;
        }
        (across * down) / (area.w * area.h)
    }
}

/// True when the page is scrolled as far down as it goes.
pub(super) fn at_end(scroll: &ScrollInfo) -> bool {
    scroll.y + scroll.viewport_height >= scroll.height - 1.0
}

/// Lists what can be clicked or typed into, as tag and text, quickly and without the
/// collector's detail. The page is not changed.
const LIST_CONTROLS: &str = "(function () {
  var found = document.querySelectorAll('a[href], area[href], button, input, select, textarea, summary, ' +
    '[role], [tabindex], [onclick], [contenteditable]');
  var keys = [];
  for (var i = 0; i < found.length && i < 5000; i++) {
    keys.push(found[i].localName + '|' + (found[i].textContent || found[i].value || '').trim().slice(0, 80));
  }
  return keys;
})";

fn list_controls(
    driver: &mut Driver,
    session: &SessionOpenResult,
    cancel: &AtomicBool,
    clock: &mut Clock,
) -> Option<Vec<String>> {
    let evaluated = timed(&mut clock.read, || {
        driver.until(cancel).eval(&PageEvalParams {
            session_id: session.session_id.clone(),
            script: LIST_CONTROLS.to_string(),
            arg: None,
            all_frames: Some(false),
            timeout_ms: Some(5_000),
        })
    });
    let value = evaluated.ok()?.results.into_iter().next()?.value?;
    serde_json::from_value(value).ok()
}

/// What going through the page produced.
pub(super) struct Walk {
    /// The reading taken at the last position, when the walk took readings.
    pub(super) reading: Option<Reading>,
    pub(super) screens: Vec<Screen>,
    /// How many times the page was scrolled.
    pub(super) steps: u64,
    /// Whether the bottom was reached. `None` when the walk could not tell.
    pub(super) reached_end: Option<bool>,
    pub(super) removals: Removals,
}

/// How long to wait at the bottom of a page for a part that is fetched only once the bottom is
/// reached, as feeds do. A page that fits on one screen is never waited for.
const PATIENCE_AT_END: Duration = Duration::from_millis(1000);

/// At the bottom of a page: waits for more to arrive, and says whether it did.
fn more_at_end(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    height: f64,
) -> Result<bool, EngineError> {
    check_cancelled(job)?;
    let scrolled = timed(&mut clock.scroll, || {
        thread::sleep(PATIENCE_AT_END);
        driver.until(job.cancel).scroll(&PageScrollParams {
            session_id: session.session_id.clone(),
            to: Some(ScrollTo::Bottom),
            by_y: None,
            wait_for: Some(ScrollWait::Height),
        })
    })?;
    Ok(scrolled.height > height + 1.0)
}

/// Goes through the page one screen at a time, the way a person scrolls, reading it and
/// taking a picture at each screen. That loads lazy content, lets reveal animations play,
/// and gives a picture of every part as it really looked.
pub(super) fn walk_with_pictures(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    screens_dir: &Path,
    max_steps: u64,
    screenshot_warning: &mut Option<Warning>,
) -> Result<Walk, EngineError> {
    fs::create_dir_all(screens_dir)?;
    let mut screens: Vec<Screen> = Vec::new();
    let mut steps = 0;
    let mut removals = Removals::default();
    loop {
        check_cancelled(job)?;
        let reading = timed(&mut clock.read, || read_frames(driver, session, job))?;
        removals.saw(
            reading
                .elements
                .iter()
                .map(|located| {
                    format!(
                        "{}|{}",
                        located.element.tag,
                        located.element.text.as_deref().unwrap_or("")
                    )
                })
                .collect(),
        );
        let scroll = &reading.scroll;
        let image = screens_dir.join(format!("{steps}.png"));
        let captured = timed(&mut clock.screenshot, || {
            screenshot(
                driver,
                job.cancel,
                &session.session_id,
                &image,
                false,
                screenshot_warning,
            )
        })?;
        if captured {
            let shown = reading
                .elements
                .iter()
                .filter(|located| located.element.state.visible)
                .filter_map(|located| {
                    let key = element_key(&located.frame_id, &located.element.selectors);
                    Some((key, located.page_box.clone()?))
                })
                .collect();
            screens.push(Screen {
                image,
                top: scroll.y,
                height: scroll.viewport_height,
                shown,
            });
        }

        let reached_end = at_end(scroll);
        // The bottom of a page that scrolls may only be the bottom for now.
        if reached_end
            && scroll.y > 0.0
            && steps < max_steps
            && more_at_end(driver, session, job, clock, scroll.height)?
        {
            steps += 1;
            continue;
        }
        if reached_end || steps >= max_steps {
            return Ok(Walk {
                reading: Some(reading),
                screens,
                steps,
                reached_end: Some(reached_end),
                removals,
            });
        }
        // A little less than a full screen, so nothing falls into the gap between two screens.
        let before = scroll.y;
        let scrolled = timed(&mut clock.scroll, || {
            driver.until(job.cancel).scroll(&PageScrollParams {
                session_id: session.session_id.clone(),
                to: None,
                by_y: Some(scroll.viewport_height * 0.9),
                wait_for: Some(ScrollWait::Animations),
            })
        })?;
        steps += 1;
        if scrolled.y <= before {
            // The page did not move: it scrolls inside a container, or not at all.
            return Ok(Walk {
                reading: Some(reading),
                screens,
                steps,
                reached_end: Some(true),
                removals,
            });
        }
    }
}

/// Goes through the page without looking at it: only scrolls, so that content which loads
/// on scroll gets loaded. With no picture to take there is nothing to read on the way and no
/// animation worth waiting for, which makes this several times quicker.
pub(super) fn walk_quickly(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    max_steps: u64,
) -> Result<Walk, EngineError> {
    let mut walk = Walk {
        reading: None,
        screens: Vec::new(),
        steps: 0,
        reached_end: None,
        removals: Removals::default(),
    };
    if let Some(keys) = list_controls(driver, session, job.cancel, clock) {
        walk.removals.saw(keys);
    }
    let mut y = 0.0;
    while walk.steps < max_steps {
        check_cancelled(job)?;
        let scrolled = timed(&mut clock.scroll, || {
            driver.until(job.cancel).scroll(&PageScrollParams {
                session_id: session.session_id.clone(),
                to: None,
                by_y: Some(job.viewport.height as f64),
                wait_for: Some(ScrollWait::Height),
            })
        })?;
        walk.steps += 1;
        if let Some(keys) = list_controls(driver, session, job.cancel, clock) {
            walk.removals.saw(keys);
        }
        if scrolled.at_end || scrolled.y <= y {
            // The bottom of a page that scrolls may only be the bottom for now.
            if scrolled.y > 0.0 && walk.steps < max_steps && more_at_end(driver, session, job, clock, scrolled.height)?
            {
                walk.steps += 1;
                y = scrolled.y;
                continue;
            }
            walk.reached_end = Some(true);
            return Ok(walk);
        }
        y = scrolled.y;
    }
    // Out of steps. Whether that was the bottom is settled by the reading taken afterwards.
    Ok(walk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scroll(y: f64, viewport_height: f64, height: f64) -> ScrollInfo {
        ScrollInfo {
            height,
            viewport_height,
            y,
        }
    }

    #[test]
    fn at_end_is_true_within_a_pixel_of_the_bottom() {
        assert!(at_end(&scroll(1600.0, 800.0, 2400.0)));
        assert!(at_end(&scroll(1599.0, 800.0, 2400.0)));
        assert!(!at_end(&scroll(1598.0, 800.0, 2400.0)));
        // A page that fits on one screen is already at its end.
        assert!(at_end(&scroll(0.0, 800.0, 600.0)));
    }

    #[test]
    fn a_screen_covers_the_share_of_a_box_it_shows() {
        let screen = Screen {
            image: PathBuf::new(),
            top: 800.0,
            height: 800.0,
            shown: HashMap::new(),
        };
        let area = |y: f64| Rect {
            x: 0.0,
            y,
            w: 100.0,
            h: 100.0,
        };
        assert_eq!(screen.coverage(&area(900.0), 1280.0), 1.0);
        assert_eq!(screen.coverage(&area(750.0), 1280.0), 0.5);
        assert_eq!(screen.coverage(&area(0.0), 1280.0), 0.0);
    }
}
