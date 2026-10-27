//! `traceqa explore`: a deterministic walk over a site by acting on it (backlog EXP-2).
//!
//! The walk is in `walk.rs` and is written against the [`Browser`] trait only. `live.rs` is the real browser behind
//! that trait. Safety is two layers: the driver refuses writes in the browser itself, and [`policy::Rules`] decides
//! before every action whether to try it at all (see the notes there).

mod data;
mod graph;
mod live;
mod model;
mod runner;
pub mod settle;
mod values;
mod walk;

#[cfg(test)]
mod tests;

pub use graph::{Edge, Graph, Skipped, State, Status, Stop, GRAPH_VERSION};
pub use live::{Live, DEFAULT_LOAD_TIMEOUT_MS};
pub use model::{Act, Acted, Browser, Effect, Hints, StepError, View, ViewComponent, ViewInstance};
pub use runner::{ExploreRun, Finished, Request, FEATURE};
pub use values::{
    CallOutcome, Field, ModelCall, ModelSettings, Recorded, SourceError, Stopped, ValueSource, Values,
    DEFAULT_MAX_MODEL_CALLS, DEFAULT_MODEL_TIMEOUT, FAILURES_BEFORE_OFF,
};
pub use walk::{explore, Explored, Progress};

use std::time::Duration;

use crate::policy::Rules;
use crate::state::Similarity;

/// The order in which the screens waiting to be explored are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Everything one action away, then two, and so on.
    Bfs,
    /// As deep as possible first.
    Dfs,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bfs => "bfs",
            Self::Dfs => "dfs",
        }
    }
}

/// The limits of a walk. Every one has a default and is written into `graph.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    pub max_states: u32,
    pub max_actions: u32,
    pub max_depth: u32,
    /// The most one action may take.
    pub action_timeout: Duration,
    /// The most the whole walk may take.
    pub timeout: Duration,
    pub max_requests_per_origin: u64,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_states: 50,
            max_actions: 300,
            max_depth: 6,
            action_timeout: Duration::from_secs(10),
            timeout: Duration::from_secs(10 * 60),
            max_requests_per_origin: 2000,
        }
    }
}

/// After this many actions in a row that used up all their time, the walk stops: the site is not answering.
pub const MAX_TIMEOUTS_IN_A_ROW: u32 = 5;

/// How many pages are read, besides the first, to find out what is on every page (the header, the footer).
pub const DEFAULT_CHROME_PAGES: usize = 6;

/// Everything a walk is told.
#[derive(Debug, Clone)]
pub struct Options {
    /// The address to start from, as the person gave it. Only http and https.
    pub start_url: String,
    pub strategy: Strategy,
    pub seed: u64,
    pub similarity: Similarity,
    pub budgets: Budgets,
    pub rules: Rules,
    /// Pages to read, besides the first, for the ids that are on every page. 0 skips it.
    pub chrome_pages: usize,
    /// A source that is asked for the text of a field before the generator is. `None`: the generator only.
    pub model: Option<ModelSettings>,
}
