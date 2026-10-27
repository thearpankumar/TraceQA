//! What the walker needs from a browser, and nothing more.
//!
//! The walk (`walk.rs`) never sees a driver, a session or a [`traceqa_common::types::Component`]. It sees a [`View`]
//! (one reading of the page) and a [`Browser`] (open an address, act on an element). That keeps the walk plain data
//! in, plain data out, so its rules (order, budgets, safety, cancel) can be tested fast, and the real browser sits
//! behind the same trait (`live.rs`) and is tested with real runs.

use std::collections::BTreeMap;

use thiserror::Error;

/// One reading of the page, as the walker needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The address the browser is at, as it reports it. Never written to a file as it is (see `walk.rs`).
    pub url: String,
    /// Every component read, hidden ones too.
    pub components: Vec<ViewComponent>,
    /// The id of the open dialog (`role=dialog` or `aria-modal`), if any.
    pub modal: Option<String>,
    /// The ids of the frames that are attached.
    pub frames: Vec<String>,
}

/// One kind of control on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewComponent {
    pub id: String,
    pub role: String,
    pub name: String,
    /// The readable path of the component in the page. Candidates are ordered by it.
    pub path: String,
    pub instances: Vec<ViewInstance>,
}

/// What the page says about a field, for choosing the text typed into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hints {
    /// The `type` attribute of an input.
    pub input_type: Option<String>,
    pub placeholder: Option<String>,
    pub autocomplete: Option<String>,
    pub required: bool,
}

/// One element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewInstance {
    pub selector: String,
    pub frame_id: String,
    pub visible: bool,
    pub enabled: bool,
    /// The `href` attribute as the page wrote it.
    pub href: Option<String>,
    /// `Some` when the element opens and closes something.
    pub expanded: Option<bool>,
    pub hints: Hints,
}

impl ViewInstance {
    /// An element a person could act on now.
    pub fn is_actionable(&self) -> bool {
        self.visible && self.enabled
    }
}

/// What the walker asks a browser to do to an element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Click,
    Type(String),
    Press(&'static str),
}

impl Act {
    /// The word the graph uses.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Type(_) => "type",
            Self::Press(_) => "press",
        }
    }
}

/// What an action did, as far as the graph records it. Only things that do not change from run to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effect {
    /// The page was navigated by the action.
    pub navigated: bool,
    /// Anything on the page changed because of the action (it navigated, or the page's own changes at rest were
    /// exceeded). Not written to the graph, because it is a measurement and not a fact of the site. The walk uses it
    /// to know whether the page still looks as it did when it was first read.
    pub changed: bool,
    pub popups: u32,
    pub dialogs: u32,
    pub downloads: u32,
}

/// An action that was performed, and the page after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acted {
    pub effect: Effect,
    pub view: View,
}

/// Why a step did not give a page.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StepError {
    /// Cancel was asked for.
    #[error("cancelled")]
    Cancelled,
    /// The element was not there in time.
    #[error("the element was not found: {0}")]
    NotFound(String),
    /// The action or the load took longer than it was allowed.
    #[error("timed out: {0}")]
    Timeout(String),
    /// This action failed. The walk goes on with the next one.
    #[error("{0}")]
    Failed(String),
    /// The browser is gone or cannot be trusted any more. The walk ends.
    #[error("{0}")]
    Fatal(String),
}

/// A browser the walker can use.
pub trait Browser {
    /// Starts from nothing (a new session with the storage state and the safety policy of the run), opens `url` and
    /// reads it. A policy the driver did not confirm is a `Fatal` error here.
    fn load(&mut self, url: &str) -> Result<View, StepError>;

    /// Acts on `instance` in the page that `load` or the last `act` left, waits for it to settle and reads again.
    fn act(&mut self, instance: &ViewInstance, act: &Act, timeout_ms: u64) -> Result<Acted, StepError>;

    /// How many requests the run has made to each origin so far. A browser that cannot count returns `None`, and the
    /// budget on requests is then not enforced (the caller says so).
    fn requests(&self) -> Option<BTreeMap<String, u64>>;
}
