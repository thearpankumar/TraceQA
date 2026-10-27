//! Where the text the walker types comes from (backlog EXP-6).
//!
//! The walker asks [`Values`] for a value and does not care who answers. Without a model that is the generator in
//! `data.rs`, which depends on the seed and the component only. With one, a [`ValueSource`] is asked first, and
//! everything that can go wrong with it is dealt with here, so that a slow, failing or hostile source can never stop
//! or steer a walk:
//!
//! - the call runs on its own thread and the walk waits for it in steps of 100 ms, so Ctrl-C and the clock end the
//!   wait whatever the source is doing;
//! - a call has a time limit, and the run's wall-clock budget cuts it short;
//! - a run has a cap on calls, and after the cap, and after three failures in a row, the generator answers;
//! - an answer that is empty, long, has a line break or does not fit the kind of field is thrown away;
//! - fields that hold secrets (passwords, card numbers) are never sent;
//! - only the field's role, label and hints are sent, with addresses redacted like driver errors;
//! - an answer is only ever text for the field it was asked for. It cannot choose an action, an address or a
//!   component: the walk and its safety rules (`policy.rs`) decide those.
//!
//! Every call is kept as a [`ModelCall`] and written to `events.jsonl` as a `model.call` event, and [`Recorded`] reads
//! those events back as a source, so that a replay of a run that used a model makes no call.
//!
//! This crate knows the trait only. No implementation that talks to a network lives here (`cargo tree -p
//! traceqa-engine` lists no HTTP client; a test checks it).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::data;
use crate::url_safety::redact_text;

/// The most calls a run makes unless told otherwise.
pub const DEFAULT_MAX_MODEL_CALLS: u32 = 50;
/// The most one call may take unless told otherwise.
pub const DEFAULT_MODEL_TIMEOUT: Duration = Duration::from_secs(20);
/// How often the walk looks at Ctrl-C and the clock while a call is out. It bounds how long a stuck call can delay
/// a cancel.
const POLL: Duration = Duration::from_millis(100);
/// After this many calls in a row that gave nothing usable, the source is not asked again in this run.
pub const FAILURES_BEFORE_OFF: u32 = 3;
/// The longest answer that is typed, in characters.
const MAX_ANSWER_CHARS: usize = 200;
/// The longest label or hint that is sent, in characters.
const MAX_SENT_CHARS: usize = 200;

/// A field the walker is about to type into, as far as a source is told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Field {
    /// The component's id. Not sent to a source.
    pub component: String,
    pub role: String,
    /// The accessible name.
    pub label: String,
    /// The `type` attribute of the input, as the page wrote it.
    pub input_type: Option<String>,
    pub placeholder: Option<String>,
    pub autocomplete: Option<String>,
    pub required: bool,
}

impl Field {
    /// What a source is given: no component id, text cut to a safe length and with addresses redacted.
    fn for_model(&self) -> Self {
        let clean = |text: &str| -> String { redact_text(text).chars().take(MAX_SENT_CHARS).collect() };
        Self {
            component: String::new(),
            role: self.role.clone(),
            label: clean(&self.label),
            input_type: self.input_type.as_deref().map(clean),
            placeholder: self.placeholder.as_deref().map(clean),
            autocomplete: self.autocomplete.as_deref().map(clean),
            required: self.required,
        }
    }

    /// What one answer is remembered by: two fields that are told the same things get the same answer. The parts are
    /// written as a JSON list, so that no text in a label can make two different fields look alike.
    pub fn key(&self) -> String {
        json!([
            self.role,
            self.label,
            self.input_type.as_deref().unwrap_or(""),
            self.placeholder.as_deref().unwrap_or(""),
            self.autocomplete.as_deref().unwrap_or(""),
        ])
        .to_string()
    }

    /// A field that holds a secret. Nothing about it is sent anywhere.
    fn is_secret(&self) -> bool {
        const WORDS: [&str; 9] = [
            "password",
            "passcode",
            "card number",
            "cvv",
            "cvc",
            "security code",
            "ssn",
            "social security",
            "one-time",
        ];
        let lower = |text: &str| text.to_lowercase();
        let hint = self.autocomplete.as_deref().map(lower).unwrap_or_default();
        self.input_type.as_deref().map(lower).as_deref() == Some("password")
            || hint.contains("password")
            || hint.starts_with("cc-")
            || hint.contains("one-time-code")
            || {
                let label = lower(&self.label);
                WORDS.iter().any(|word| label.contains(word))
            }
    }

    fn to_json(&self) -> Value {
        json!({
            "role": self.role,
            "label": self.label,
            "inputType": self.input_type,
            "placeholder": self.placeholder,
            "autocomplete": self.autocomplete,
            "required": self.required,
        })
    }
}

/// A source could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(pub String);

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Something that can suggest the text for a field. It is called on its own thread and may take as long as it likes:
/// the walk does not wait for it past the limits.
pub trait ValueSource: fmt::Debug + Send + Sync {
    /// Its name, for the log.
    fn name(&self) -> &str;
    fn value(&self, field: &Field) -> Result<String, SourceError>;
}

/// How a run uses a source.
#[derive(Debug, Clone)]
pub struct ModelSettings {
    pub source: Arc<dyn ValueSource>,
    /// The most calls the run makes.
    pub max_calls: u32,
    /// The most one call may take.
    pub timeout: Duration,
}

/// What came of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallOutcome {
    /// The answer was typed.
    Used,
    /// The source answered and the answer was thrown away.
    Rejected,
    /// The source gave an error, or stopped without an answer.
    Failed,
    /// The call took longer than it was allowed.
    TimedOut,
    /// The run was cancelled, or ran out of time, while the call was out.
    Stopped,
    /// The run's cap on calls was reached. Written once, at the first field that was not asked.
    OverCap,
}

impl CallOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Used => "used",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::TimedOut => "timed-out",
            Self::Stopped => "stopped",
            Self::OverCap => "over-cap",
        }
    }
}

/// One call, as written to `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCall {
    pub source: String,
    /// What the call was told, after redaction.
    pub request: Field,
    pub outcome: CallOutcome,
    /// What the source said, when it said something (also when it was thrown away).
    pub answer: Option<String>,
    /// Why it was not used, or what else is worth knowing.
    pub note: Option<String>,
    pub millis: u64,
}

impl ModelCall {
    /// One line for the person watching.
    pub fn summary(&self) -> String {
        let end = match &self.note {
            Some(note) => format!("{} ({note})", self.outcome.as_str()),
            None => self.outcome.as_str().to_string(),
        };
        format!("model {} for {:?}: {end}", self.source, self.request.label)
    }

    /// The `data` of the `model.call` event.
    pub fn data(&self) -> Value {
        json!({
            "source": self.source,
            "key": self.request.key(),
            "request": self.request.to_json(),
            "outcome": self.outcome.as_str(),
            "answer": self.answer,
            "note": self.note,
            "millis": self.millis,
        })
    }
}

/// Why a value could not be had: the walk must end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    Cancelled,
    OutOfTime,
}

struct Model {
    settings: ModelSettings,
    made: u32,
    failures_in_a_row: u32,
    off: bool,
    said_cap: bool,
    /// Answers already used in this run, by [`Field::key`]: the same field is not asked twice.
    answers: BTreeMap<String, String>,
}

/// The text for every field the walk types into, and the record of the calls that were made for it.
pub struct Values {
    seed: u64,
    model: Option<Model>,
    calls: Vec<ModelCall>,
}

impl Values {
    pub fn new(seed: u64, settings: Option<ModelSettings>) -> Self {
        Self {
            seed,
            model: settings.map(|settings| Model {
                settings,
                made: 0,
                failures_in_a_row: 0,
                off: false,
                said_cap: false,
                answers: BTreeMap::new(),
            }),
            calls: Vec::new(),
        }
    }

    pub fn calls(&self) -> &[ModelCall] {
        &self.calls
    }

    pub fn take_calls(&mut self) -> Vec<ModelCall> {
        std::mem::take(&mut self.calls)
    }

    /// The text to type into `field`. `time_left` is what the run's wall-clock budget has left.
    pub fn value_for(&mut self, field: &Field, cancel: &AtomicBool, time_left: Duration) -> Result<String, Stopped> {
        let fallback = data::value_for(self.seed, &field.component, &field.label);
        let Some(model) = self.model.as_mut() else {
            return Ok(fallback);
        };
        if model.off || field.is_secret() {
            return Ok(fallback);
        }
        let sent = field.for_model();
        let key = sent.key();
        if let Some(answer) = model.answers.get(&key) {
            return Ok(answer.clone());
        }
        let source_name = model.settings.source.name().to_string();
        if model.made >= model.settings.max_calls {
            if !model.said_cap {
                model.said_cap = true;
                self.calls.push(ModelCall {
                    source: source_name,
                    request: sent,
                    outcome: CallOutcome::OverCap,
                    answer: None,
                    note: Some(format!("{} calls made", model.made)),
                    millis: 0,
                });
            }
            return Ok(fallback);
        }
        model.made += 1;

        let started = Instant::now();
        let ended = ask(
            &model.settings.source,
            sent.clone(),
            cancel,
            model.settings.timeout,
            time_left,
        );
        let millis = started.elapsed().as_millis() as u64;
        let mut call = ModelCall {
            source: source_name,
            request: sent.clone(),
            outcome: CallOutcome::Failed,
            answer: None,
            note: None,
            millis,
        };
        let mut stop = None;
        let mut typed = None;
        match ended {
            Ended::Answered(Ok(answer)) => match check(&sent, &answer) {
                Ok(good) => {
                    call.outcome = CallOutcome::Used;
                    // What was typed, as it was typed: a replay must type the same.
                    call.answer = Some(good.clone());
                    model.answers.insert(key, good.clone());
                    typed = Some(good);
                }
                Err(why) => {
                    call.outcome = CallOutcome::Rejected;
                    call.answer = Some(redact_text(&answer).chars().take(MAX_ANSWER_CHARS).collect());
                    call.note = Some(why.to_string());
                }
            },
            Ended::Answered(Err(error)) => {
                call.note = Some(redact_text(&error.0).chars().take(MAX_ANSWER_CHARS).collect());
            }
            Ended::Died => call.note = Some("the source stopped without an answer".to_string()),
            Ended::TimedOut => {
                call.outcome = CallOutcome::TimedOut;
                call.note = Some(format!("no answer in {} s", model.settings.timeout.as_secs_f32()));
            }
            Ended::Stopped(why) => {
                call.outcome = CallOutcome::Stopped;
                call.note = Some(
                    match why {
                        Stopped::Cancelled => "the run was cancelled",
                        Stopped::OutOfTime => "the run's time ran out",
                    }
                    .to_string(),
                );
                stop = Some(why);
            }
        }
        if call.outcome == CallOutcome::Used {
            model.failures_in_a_row = 0;
        } else if stop.is_none() {
            model.failures_in_a_row += 1;
            if model.failures_in_a_row >= FAILURES_BEFORE_OFF {
                model.off = true;
                let note = call.note.take().map(|note| format!("{note}; ")).unwrap_or_default();
                call.note = Some(format!(
                    "{note}switched off for the rest of the run after {FAILURES_BEFORE_OFF} calls in a row without a usable answer"
                ));
            }
        }
        self.calls.push(call);
        match stop {
            Some(why) => Err(why),
            None => Ok(typed.unwrap_or(fallback)),
        }
    }
}

enum Ended {
    Answered(Result<String, SourceError>),
    TimedOut,
    Stopped(Stopped),
    /// The thread ended without sending anything (the source panicked).
    Died,
}

/// Calls the source on its own thread and waits for it, looking at Ctrl-C and the clock every [`POLL`].
///
/// A call that is given up on is not stopped: its thread is left to finish by itself and its answer goes nowhere.
/// For a command that ends when the run ends that costs nothing. A long-lived process that makes many runs would
/// collect one waiting thread per stuck call, so it needs a source that can be cancelled (a client with its own
/// timeout) or this to run calls in a pool with a bound. See the notes on EXP-6 in `docs/backlog.md`.
fn ask(
    source: &Arc<dyn ValueSource>,
    field: Field,
    cancel: &AtomicBool,
    call_limit: Duration,
    time_left: Duration,
) -> Ended {
    if cancel.load(Ordering::SeqCst) {
        return Ended::Stopped(Stopped::Cancelled);
    }
    if time_left.is_zero() {
        return Ended::Stopped(Stopped::OutOfTime);
    }
    let (send, receive) = mpsc::channel();
    let source = Arc::clone(source);
    let spawned = std::thread::Builder::new()
        .name("traceqa-model-call".to_string())
        .spawn(move || {
            let _ = send.send(source.value(&field));
        });
    if spawned.is_err() {
        return Ended::Died;
    }
    let started = Instant::now();
    loop {
        match receive.recv_timeout(POLL) {
            Ok(answer) => return Ended::Answered(answer),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ended::Died,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.load(Ordering::SeqCst) {
                    return Ended::Stopped(Stopped::Cancelled);
                }
                let waited = started.elapsed();
                if waited >= time_left {
                    return Ended::Stopped(Stopped::OutOfTime);
                }
                if waited >= call_limit {
                    return Ended::TimedOut;
                }
            }
        }
    }
}

/// Is `answer` something to type into `field`? The cleaned answer, or why not.
fn check(field: &Field, answer: &str) -> Result<String, &'static str> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("the answer was empty");
    }
    if answer.chars().count() > MAX_ANSWER_CHARS {
        return Err("the answer was too long");
    }
    if answer.chars().any(char::is_control) {
        return Err("the answer had a line break or another control character");
    }
    let kind = field.input_type.as_deref().unwrap_or("").to_lowercase();
    let fits = match kind.as_str() {
        "email" => answer.contains('@') && !answer.contains(char::is_whitespace),
        "number" | "range" => answer.parse::<f64>().is_ok(),
        "tel" => answer.chars().all(|c| c.is_ascii_digit() || " +-()".contains(c)),
        "url" => !answer.contains(char::is_whitespace),
        _ => true,
    };
    if fits {
        Ok(answer.to_string())
    } else {
        Err("the answer did not fit the kind of field")
    }
}

/// A source made of the answers a run wrote down. A walk with this asks no model: the same fields get the same
/// answers, and a field the run never asked about is an error, so the generator answers for it.
#[derive(Debug, Default)]
pub struct Recorded {
    answers: BTreeMap<String, String>,
}

impl Recorded {
    /// Reads the `model.call` events of an `events.jsonl`. Only calls whose answer was used count. Lines that are
    /// not events of that kind are ignored.
    pub fn from_events(text: &str) -> Self {
        let mut answers = BTreeMap::new();
        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if event["type"] != "model.call" || event["data"]["outcome"] != "used" {
                continue;
            }
            if let (Some(key), Some(answer)) = (event["data"]["key"].as_str(), event["data"]["answer"].as_str()) {
                answers.entry(key.to_string()).or_insert_with(|| answer.to_string());
            }
        }
        Self { answers }
    }

    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        Ok(Self::from_events(&std::fs::read_to_string(path)?))
    }

    pub fn len(&self) -> usize {
        self.answers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.answers.is_empty()
    }
}

impl ValueSource for Recorded {
    fn name(&self) -> &str {
        "recorded"
    }

    fn value(&self, field: &Field) -> Result<String, SourceError> {
        self.answers
            .get(&field.key())
            .cloned()
            .ok_or_else(|| SourceError("no recorded answer for this field".to_string()))
    }
}
