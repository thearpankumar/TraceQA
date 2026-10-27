//! The real browser behind [`Browser`]: a driver, one session at a time, the safety policy of the run.
//!
//! Every `load` starts a new session (a new browser context with the storage state), so nothing the last visit left
//! in the page, cookies or storage leaks into the next. A session is only used for acting if the driver says back
//! the policy it was asked to keep (`policy::enforced`).

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use traceqa_common::contracts::driver_protocol::method;
use traceqa_common::types::{
    ActPolicy, ActionKind, BrowserTarget, Component, ElementTarget, PageActParams, PageGotoParams, SessionOpenParams,
    SessionOpenResult, StorageState, Viewport, WaitUntil,
};

use super::model::{Act, Acted, Browser, Effect, Hints, StepError, View, ViewComponent, ViewInstance};
use super::settle::{settled, Reader, MAX_READS, PAUSE_MS};
use crate::driver::{DriverError, Drivers, ErrorKind, DEFAULT_TIMEOUT_MS};
use crate::extract::read_live;
use crate::policy::enforced;
use crate::EngineError;

pub struct Live<'a> {
    drivers: &'a mut Drivers,
    target: BrowserTarget,
    viewport: Viewport,
    storage_state: Option<StorageState>,
    policy: ActPolicy,
    cancel: &'a AtomicBool,
    /// How long a page may take to load, in milliseconds.
    load_timeout_ms: u64,
    session: Option<SessionOpenResult>,
    /// Requests made in the sessions that are closed, by origin.
    earlier: BTreeMap<String, u64>,
    /// Requests made so far in the session that is open, by origin.
    current: BTreeMap<String, u64>,
    /// The driver has reported counts at least once. A driver that cannot count sends none, and the budget on
    /// requests is then not enforced.
    counts_known: bool,
}

impl<'a> Live<'a> {
    pub fn new(
        drivers: &'a mut Drivers,
        target: BrowserTarget,
        viewport: Viewport,
        storage_state: Option<StorageState>,
        policy: ActPolicy,
        cancel: &'a AtomicBool,
        load_timeout_ms: u64,
    ) -> Self {
        Self {
            drivers,
            target,
            viewport,
            storage_state,
            policy,
            cancel,
            load_timeout_ms,
            session: None,
            earlier: BTreeMap::new(),
            current: BTreeMap::new(),
            counts_known: false,
        }
    }

    /// The browser the session ran in, for the run folder. `None` before the first `load`.
    pub fn browser(&self) -> Option<traceqa_common::types::BrowserInfo> {
        self.session.as_ref().map(|session| session.browser.clone())
    }

    /// Closes the session that is open, if any. Safe to call more than once.
    pub fn close(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        for (origin, count) in std::mem::take(&mut self.current) {
            *self.earlier.entry(origin).or_default() += count;
        }
        if let Ok(driver) = self.drivers.for_browser(&self.target.browser) {
            if !driver.is_lost() {
                if let Err(error) = driver.close_session(&session.session_id) {
                    eprintln!("traceqa: could not close the {} session: {error}", self.target.browser);
                }
            }
        }
    }

    /// The session is gone together with the driver that held it, so there is nothing to close. Its requests still
    /// count.
    fn forget_session(&mut self) {
        self.session = None;
        for (origin, count) in std::mem::take(&mut self.current) {
            *self.earlier.entry(origin).or_default() += count;
        }
    }

    /// A call failed. When that loses the driver (see `DriverError::loses_driver_calling`), its sessions are gone.
    fn call_failed(&mut self, error: &DriverError, method: &str) {
        if error.loses_driver_calling(method) {
            self.forget_session();
        }
    }

    fn note_counts(&mut self, counts: &std::collections::HashMap<String, i64>) {
        if counts.is_empty() {
            return;
        }
        self.counts_known = true;
        self.current = counts
            .iter()
            .map(|(origin, count)| (origin.clone(), u64::try_from(*count).unwrap_or(0)))
            .collect();
    }

    fn open(&mut self) -> Result<SessionOpenResult, StepError> {
        let driver = self
            .drivers
            .for_browser(&self.target.browser)
            .map_err(|e| step_error(&e.into()))?;
        let opened = driver
            .until(self.cancel)
            .open_session(&SessionOpenParams {
                target: self.target.clone(),
                viewport: self.viewport.clone(),
                locale: None,
                storage_state: self.storage_state.clone(),
                policy: Some(self.policy.clone()),
            })
            .map_err(|e| step_error(&e.into()))?;
        // A session that is not known to be protected must not be acted on.
        if !enforced(&self.policy, opened.policy.as_ref()) {
            let _ = driver.close_session(&opened.session_id);
            return Err(StepError::Fatal(
                "the driver did not confirm the safety policy, so nothing was done. Use a driver that keeps one".into(),
            ));
        }
        if opened.capabilities.action_kinds.is_empty() {
            let _ = driver.close_session(&opened.session_id);
            return Err(StepError::Fatal(format!(
                "this browser cannot be acted on by its driver ({}). Use chromium, firefox or webkit",
                self.target.browser
            )));
        }
        Ok(opened)
    }

    /// One reading of the page, as it is this moment.
    fn read_once(&mut self, timeout_ms: u64) -> Result<View, StepError> {
        let session = self
            .session
            .clone()
            .ok_or_else(|| StepError::Fatal("no session is open".into()))?;
        let driver = self
            .drivers
            .for_browser(&self.target.browser)
            .map_err(|e| step_error(&e.into()))?;
        let reading = match read_live(driver, &session, self.cancel, Some(timeout_ms)) {
            Ok(reading) => reading,
            Err(error) => {
                if let EngineError::Driver(driver_error) = &error {
                    self.call_failed(driver_error, method::PAGE_EVAL);
                }
                if error.is_cancelled() {
                    self.forget_session();
                }
                return Err(step_error(&error));
            }
        };
        Ok(view_of(reading.url, reading.frame_ids, &reading.components))
    }
    /// The page once it has stopped appearing (see `settle`).
    fn read(&mut self, timeout_ms: u64) -> Result<View, StepError> {
        settled(&mut LiveReader { live: self, timeout_ms }, MAX_READS)
    }
}

/// `Live` as the thing `settled` reads and waits with.
struct LiveReader<'b, 'a> {
    live: &'b mut Live<'a>,
    timeout_ms: u64,
}

impl Reader for LiveReader<'_, '_> {
    fn read(&mut self) -> Result<View, StepError> {
        self.live.read_once(self.timeout_ms)
    }

    fn pause(&mut self) -> Result<(), StepError> {
        // In short slices, so a cancel is seen at once.
        let mut left = PAUSE_MS;
        while left > 0 {
            if self.live.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(StepError::Cancelled);
            }
            let slice = left.min(50);
            std::thread::sleep(std::time::Duration::from_millis(slice));
            left -= slice;
        }
        Ok(())
    }
}

impl Browser for Live<'_> {
    fn load(&mut self, url: &str) -> Result<View, StepError> {
        self.close();
        let session = self.open()?;
        let id = session.session_id.clone();
        self.session = Some(session);
        let driver = self
            .drivers
            .for_browser(&self.target.browser)
            .map_err(|e| step_error(&e.into()))?;
        let went = driver
            .until(self.cancel)
            .goto(&PageGotoParams {
                session_id: id,
                url: url.to_string(),
                wait_until: Some(WaitUntil::Settled),
                timeout_ms: Some(self.load_timeout_ms),
            })
            .map_err(|error| {
                let step = step_error(&error.clone().into());
                (error, step)
            });
        let went = match went {
            Ok(went) => went,
            Err((error, step)) => {
                self.call_failed(&error, method::PAGE_GOTO);
                self.close();
                return Err(step);
            }
        };
        self.note_counts(&went.request_counts);
        let view = self.read(self.load_timeout_ms);
        if view.is_err() {
            self.close();
        }
        view
    }

    fn act(&mut self, instance: &ViewInstance, act: &Act, timeout_ms: u64) -> Result<Acted, StepError> {
        let session_id = self
            .session
            .as_ref()
            .map(|session| session.session_id.clone())
            .ok_or_else(|| StepError::Fatal("no session is open".into()))?;
        let (action, value) = match act {
            Act::Click => (ActionKind::Click, None),
            Act::Type(text) => (ActionKind::Type, Some(text.clone())),
            Act::Press(key) => (ActionKind::Press, Some((*key).to_string())),
        };
        let params = PageActParams {
            session_id,
            action,
            target: ElementTarget {
                selector: instance.selector.clone(),
                frame_id: Some(instance.frame_id.clone()),
            },
            value,
            timeout_ms: std::num::NonZeroU64::new(timeout_ms),
        };
        let driver = self
            .drivers
            .for_browser(&self.target.browser)
            .map_err(|e| step_error(&e.into()))?;
        let result = match driver.until(self.cancel).act(&params) {
            Ok(result) => result,
            Err(error) => {
                self.call_failed(&error, method::PAGE_ACT);
                return Err(step_error(&error.into()));
            }
        };
        self.note_counts(&result.request_counts);
        let navigated = result.navigated.is_some();
        let effect = Effect {
            navigated,
            changed: navigated || result.dom_changed.unwrap_or(false),
            popups: count(result.popups.len()),
            dialogs: count(result.dialogs.len()),
            downloads: count(result.downloads.len()),
        };
        // Reading the page is part of the action, so it is given the action's time and no more.
        let view = self.read(timeout_ms)?;
        Ok(Acted { effect, view })
    }

    fn requests(&self) -> Option<BTreeMap<String, u64>> {
        if !self.counts_known {
            return None;
        }
        let mut total = self.earlier.clone();
        for (origin, count) in &self.current {
            *total.entry(origin.clone()).or_default() += count;
        }
        Some(total)
    }
}

impl Drop for Live<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// How a failure of the driver or of the reading is told to the walk.
fn step_error(error: &EngineError) -> StepError {
    match error {
        EngineError::Cancelled => StepError::Cancelled,
        EngineError::Driver(driver) => match driver.kind() {
            ErrorKind::Timeout => StepError::Timeout(driver.message().to_string()),
            ErrorKind::ElementNotFound => StepError::NotFound(driver.message().to_string()),
            // These say what happened to one action or one page; the next one may work.
            ErrorKind::NavigationFailed | ErrorKind::InvalidParams | ErrorKind::EvalFailed | ErrorKind::BlockedUrl => {
                StepError::Failed(driver.message().to_string())
            }
            ErrorKind::Internal | ErrorKind::Other(_) => StepError::Failed(driver.message().to_string()),
            // The driver or the session is gone, or cannot do this: nothing more can be done with it.
            _ => StepError::Fatal(driver.message().to_string()),
        },
        EngineError::Invalid(message) => StepError::Failed(message.clone()),
        EngineError::Io(error) => StepError::Fatal(error.to_string()),
    }
}

/// The dialog a component sits in, as `role "name"`, from the readable path of the component. A dialog is not a
/// component itself (it is not interactive), but every control inside it has it in its path. This is what the screen
/// is told apart by while a dialog is open: it has no id of its own, so the marker is its role and name.
fn dialog_in(path: &str) -> Option<String> {
    path.split(" > ")
        .find(|step| step.starts_with("dialog ") || step.starts_with("alertdialog "))
        .map(str::to_string)
}

/// The walker's view of a reading.
fn view_of(url: String, frame_ids: Vec<String>, components: &[Component]) -> View {
    let modal = components
        .iter()
        .filter(|component| component.instances.iter().any(|i| i.state.visible))
        .filter_map(|component| dialog_in(&component.path))
        .min();
    View {
        url,
        modal,
        frames: frame_ids,
        components: components
            .iter()
            .map(|component| ViewComponent {
                id: component.id.clone(),
                role: component.role.clone(),
                name: component.name.clone(),
                path: component.path.clone(),
                instances: component
                    .instances
                    .iter()
                    .filter_map(|instance| {
                        Some(ViewInstance {
                            selector: instance.selectors.first()?.clone(),
                            frame_id: instance.frame_id.clone(),
                            visible: instance.state.visible,
                            enabled: instance.state.enabled,
                            href: instance.attrs.get("href").cloned(),
                            expanded: instance.state.expanded,
                            hints: Hints {
                                input_type: instance.attrs.get("type").cloned(),
                                placeholder: instance.attrs.get("placeholder").cloned(),
                                autocomplete: instance.attrs.get("autocomplete").cloned(),
                                required: instance.state.required.unwrap_or(false),
                            },
                        })
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The time a page may take to load when nothing says otherwise.
pub const DEFAULT_LOAD_TIMEOUT_MS: u64 = DEFAULT_TIMEOUT_MS;

#[cfg(test)]
mod tests {
    use super::dialog_in;

    #[test]
    fn a_control_inside_a_dialog_names_the_dialog() {
        assert_eq!(
            dialog_in("main > dialog \"Help\" > list").as_deref(),
            Some("dialog \"Help\"")
        );
        assert_eq!(
            dialog_in("alertdialog \"Sure?\"").as_deref(),
            Some("alertdialog \"Sure?\"")
        );
    }

    #[test]
    fn a_control_outside_any_dialog_names_none() {
        assert_eq!(dialog_in("main > list \"products\" > listitem"), None);
        assert_eq!(dialog_in(""), None);
        // A name that merely starts with the word is not a dialog.
        assert_eq!(dialog_in("main > dialogue \"x\""), None);
    }
}
