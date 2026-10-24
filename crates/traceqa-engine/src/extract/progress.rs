//! Recording what happens during an extraction: events, and where the time went.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use traceqa_protocol::sync::LockExt;
use traceqa_protocol::types::{Event, Timings};
use traceqa_recorder::RunWriter;

/// The run folder's writer and the live listener. Every target of a run shares them.
pub(crate) struct Sink<'a> {
    pub writer: RunWriter,
    pub listener: &'a mut (dyn FnMut(&Event) + Send),
    /// Set once an abort has written the final state: nothing may be added after `run.finished`.
    pub sealed: bool,
}

/// Records events in the run folder and passes them to whoever is listening live. Targets
/// running side by side each hold a copy, and an event is written and delivered as one step,
/// so the listener sees events in the order they have in `events.jsonl`.
#[derive(Clone, Copy)]
pub(crate) struct Progress<'a, 'b> {
    pub sink: &'a Mutex<Sink<'b>>,
}

impl Progress<'_, '_> {
    pub fn emit(&self, kind: &str, target: Option<&str>, message: String, data: Value) {
        let mut guard = self.sink.lock_ok();
        let sink = &mut *guard;
        if sink.sealed {
            return;
        }
        match sink.writer.event(kind, target, message, data) {
            Ok(event) => (sink.listener)(&event),
            Err(error) => eprintln!("traceqa: could not write events.jsonl: {error}"),
        }
    }

    /// Uses the run folder's writer for something other than an event.
    pub fn with_writer<T>(&self, work: impl FnOnce(&RunWriter) -> T) -> T {
        work(&self.sink.lock_ok().writer)
    }
}

/// Where the time of one extraction went.
#[derive(Default)]
pub(super) struct Clock {
    pub(super) open: Duration,
    pub(super) load: Duration,
    pub(super) read: Duration,
    pub(super) scroll: Duration,
    pub(super) screenshot: Duration,
    pub(super) crop: Duration,
    pub(super) screens: u64,
}

impl Clock {
    pub(super) fn timings(&self) -> Timings {
        let ms = crate::util::ms;
        Timings {
            open_ms: ms(self.open),
            load_ms: ms(self.load),
            read_ms: ms(self.read),
            scroll_ms: ms(self.scroll),
            screenshot_ms: ms(self.screenshot),
            crop_ms: ms(self.crop),
            screens: Some(self.screens),
        }
    }
}

/// Does `work` and adds the time it took to `spent`.
pub(super) fn timed<T>(spent: &mut Duration, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let done = work();
    *spent += started.elapsed();
    done
}
