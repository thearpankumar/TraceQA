//! `traceqa replay`: perform again the actions of a graph, and say for each one whether it led where it did (EXP-8).
//!
//! Like the walk, this is written against the [`Browser`] trait only, so its rules (order, what is blocked, what
//! happens after a break, the clock, cancel) are tested without a browser, and the real browser sits behind the same
//! trait. Two ways to ask:
//!
//! - [`Mode::Chain`]: edges that follow one another from the start page, done in one session. The page after each one
//!   must be the screen the graph says. The first one that is not ends the chain, because after it the page is not
//!   known, and the rest are `not-run`.
//! - [`Mode::All`]: every edge of the graph, each from the screen it was made on. The screen is reopened by its
//!   address, or, for one that has no address of its own (a dialog), by performing the recorded way to it.
//!
//! A component is found again by its id, and a page is the screen the graph says by the rule the walk used
//! ([`Screens::matches`]), built from the graph itself. Nothing is retried to make a result come out right: a step
//! that does not lead where it did is reported as it is.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::graph::Status;
use super::model::{Act, Acted, Browser, StepError, View, ViewInstance};
use super::replay_doc::{ActionKind, Doc, DocError};
use super::replay_out::ReplayEvent;
use super::walk::{actionable, key_for, kind_of, visible_ids, Kind, PRESS_TIMEOUT_MS};
use crate::policy::{Candidate, Decision, Rules};
use crate::state::{Look, Screens};
use crate::url_safety::{redact, redact_text};

/// Which edges to perform, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// These edges, by position, in this order, in one session from the start page. Each starts where the one before
    /// it ended, and the first starts on the start page ([`chain_of`] checks it).
    Chain(Vec<usize>),
    /// Every edge, each from the screen it was made on.
    All,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chain(_) => "chain",
            Self::All => "all",
        }
    }
}

/// What came of one edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The page is the screen the graph says.
    Reproduced,
    /// The action was made (or failed) and the page is not that screen.
    Diverged,
    /// The component is not on the page, so nothing was done.
    Missing,
    /// The rules leave it alone, or there is nothing to type.
    Blocked,
    /// It was not tried: an earlier step broke, the screen could not be reached, or the replay ended first.
    NotRun,
    /// The walk itself did not complete it, so there is nothing to compare with.
    RecordedFailed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reproduced => "reproduced",
            Self::Diverged => "diverged",
            Self::Missing => "missing",
            Self::Blocked => "blocked",
            Self::NotRun => "not-run",
            Self::RecordedFailed => "recorded-failed",
        }
    }
}

/// One edge and its result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The position of the edge in the graph.
    pub edge: usize,
    pub outcome: Outcome,
    /// For `diverged`: the screen the page was (`sN`), `unknown` for a page the graph has no screen like, or nothing
    /// when the action did not complete.
    pub actual: Option<String>,
    /// Why, in a sentence, when there is something to say.
    pub note: Option<String>,
}

/// Why the replay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Every requested edge was dealt with.
    Complete,
    Timeout,
    MaxRequests,
    Cancelled,
    /// The browser failed. The message is in [`Replayed::error`].
    Error,
}

impl Ended {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Timeout => "timeout",
            Self::MaxRequests => "max-requests",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

/// The limits and the rules of a replay.
#[derive(Debug, Clone)]
pub struct Options {
    pub rules: Rules,
    /// The most one action may take.
    pub action_timeout: Duration,
    /// The most the whole replay may take.
    pub timeout: Duration,
    pub max_requests_per_origin: u64,
}

/// What a replay came to: one step per requested edge, in the order asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    pub steps: Vec<Step>,
    pub ended: Ended,
    /// How many actions were made, the ones that only reopened a screen included.
    pub actions: u32,
    pub error: Option<String>,
}

/// The edges of a chain given by name, checked: each leads to the screen the next one starts on, the first starts on
/// the start page, and all of them worked in the walk.
pub fn chain_of(doc: &Doc, names: &[String]) -> Result<Vec<usize>, DocError> {
    if names.is_empty() {
        return Err(DocError("no edges were named".to_string()));
    }
    let mut chain = Vec::with_capacity(names.len());
    let mut at = 0usize;
    for name in names {
        let Some(position) = doc.edge_index(name) else {
            return Err(DocError(format!("{name} is not an edge of the graph")));
        };
        let edge = &doc.edges[position];
        if edge.status != Status::Ok {
            return Err(DocError(format!(
                "{name} did not work in the walk (it was {}), so there is nowhere to replay it to",
                edge.status.as_str()
            )));
        }
        if edge.from != at {
            return Err(DocError(format!(
                "{name} starts on s{} and the chain is on s{at}: edges named together must follow one another from s0",
                edge.from
            )));
        }
        at = edge.to.unwrap_or(at);
        chain.push(position);
    }
    Ok(chain)
}

/// The fewest edges that lead from the start page to screen `to`, as positions, without the edges that typed text
/// (doing a write again could do its harm again). `None` when the graph has no such way.
pub fn shortest_way(doc: &Doc, to: usize) -> Option<Vec<usize>> {
    let mut via: Vec<Option<usize>> = vec![None; doc.states.len()];
    let mut seen = BTreeSet::from([0usize]);
    let mut queue = VecDeque::from([0usize]);
    while let Some(here) = queue.pop_front() {
        if here == to {
            break;
        }
        for (position, edge) in doc.edges.iter().enumerate() {
            let (true, Status::Ok, Some(next)) = (edge.from == here, edge.status, edge.to) else {
                continue;
            };
            if edge.action == ActionKind::Type || !seen.insert(next) {
                continue;
            }
            via[next] = Some(position);
            queue.push_back(next);
        }
    }
    if !seen.contains(&to) {
        return None;
    }
    let mut way = Vec::new();
    let mut at = to;
    while at != 0 {
        let position = via[at]?;
        way.push(position);
        at = doc.edges[position].from;
    }
    way.reverse();
    Some(way)
}

pub type Progress<'a> = &'a mut dyn FnMut(&ReplayEvent);

/// Performs `mode` on `doc` with `browser`.
pub fn replay(
    browser: &mut dyn Browser,
    doc: &Doc,
    mode: &Mode,
    options: &Options,
    cancel: &AtomicBool,
    progress: Progress,
) -> Replayed {
    let requested: Vec<usize> = match mode {
        Mode::Chain(edges) => edges.clone(),
        Mode::All => (0..doc.edges.len()).collect(),
    };
    let screens = match doc.screens() {
        Ok(screens) => screens,
        Err(error) => {
            let steps: Vec<Step> = requested
                .iter()
                .map(|&edge| not_run(edge, "the graph cannot be used"))
                .collect();
            for step in &steps {
                progress(&ReplayEvent::Step {
                    edge: &doc.edges[step.edge],
                    step,
                });
            }
            return Replayed {
                steps,
                ended: Ended::Error,
                actions: 0,
                error: Some(error.to_string()),
            };
        }
    };
    let mut replayer = Replayer {
        browser,
        doc,
        options,
        cancel,
        progress,
        started: Instant::now(),
        screens,
        actions: 0,
        needs_way: BTreeSet::new(),
        unreachable: BTreeSet::new(),
        at: None,
        steps: Vec::with_capacity(requested.len()),
        left_over: "the replay ended before it".to_string(),
    };
    let outcome = match mode {
        Mode::Chain(edges) => replayer.chain(edges),
        Mode::All => replayer.all(),
    };
    let (ended, error) = match outcome {
        Ok(()) => (Ended::Complete, None),
        Err(Halt::Stop(ended)) => (ended, None),
        Err(Halt::Error(message)) => (Ended::Error, Some(message)),
    };
    // Every requested edge has a step, in the order asked.
    let why = replayer.left_over.clone();
    for &edge in &requested[replayer.steps.len()..] {
        replayer.push(not_run(edge, &why));
    }
    Replayed {
        steps: replayer.steps,
        ended,
        actions: replayer.actions,
        error,
    }
}

fn not_run(edge: usize, why: &str) -> Step {
    Step {
        edge,
        outcome: Outcome::NotRun,
        actual: None,
        note: Some(why.to_string()),
    }
}

enum Halt {
    Stop(Ended),
    Error(String),
}

fn halt(error: StepError) -> Halt {
    match error {
        StepError::Cancelled => Halt::Stop(Ended::Cancelled),
        other => Halt::Error(other.to_string()),
    }
}

/// What `perform` found out, and the page after the action when there is one.
struct Done {
    step: Step,
    view: Option<View>,
    /// Anything on the page changed because of the action. When nothing did, the page is still the one it was.
    changed: bool,
}

struct Replayer<'a> {
    browser: &'a mut dyn Browser,
    doc: &'a Doc,
    options: &'a Options,
    cancel: &'a AtomicBool,
    progress: Progress<'a>,
    started: Instant,
    screens: Screens,
    actions: u32,
    /// Screens whose address does not bring them back (a dialog, an open drawer): they are reopened by performing the
    /// recorded way to them, without trying the address first.
    needs_way: BTreeSet<usize>,
    unreachable: BTreeSet<usize>,
    /// The screen the browser is on, and the page, when the last action changed nothing and so the page is still as
    /// the screen was found.
    at: Option<(usize, View)>,
    steps: Vec<Step>,
    /// What to say about the requested edges that were never reached.
    left_over: String,
}

impl Replayer<'_> {
    fn push(&mut self, step: Step) {
        (self.progress)(&ReplayEvent::Step {
            edge: &self.doc.edges[step.edge],
            step: &step,
        });
        self.steps.push(step);
    }

    fn say(&mut self, line: &str) {
        (self.progress)(&ReplayEvent::Line(line));
    }

    /// The things that end a replay wherever it is: cancel, the clock and the number of requests.
    fn check_limits(&self) -> Result<(), Halt> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(Halt::Stop(Ended::Cancelled));
        }
        if self.started.elapsed() >= self.options.timeout {
            return Err(Halt::Stop(Ended::Timeout));
        }
        if let Some(counts) = self.browser.requests() {
            if counts.values().any(|&n| n >= self.options.max_requests_per_origin) {
                return Err(Halt::Stop(Ended::MaxRequests));
            }
        }
        Ok(())
    }

    fn chain(&mut self, edges: &[usize]) -> Result<(), Halt> {
        let Some(&first) = edges.first() else {
            return Ok(());
        };
        let mut view = match self.open_start()? {
            Ok(view) => view,
            Err(why) => {
                self.push(Step {
                    edge: first,
                    outcome: Outcome::Diverged,
                    actual: None,
                    note: Some(why),
                });
                self.left_over = format!("{} did not reproduce", self.doc.edges[first].id);
                return Ok(());
            }
        };
        for &edge in edges {
            self.check_limits()?;
            let done = self.perform(edge, &view)?;
            let reproduced = done.step.outcome == Outcome::Reproduced;
            self.push(done.step);
            match done.view {
                Some(next) if reproduced => view = next,
                _ => {
                    self.left_over = format!("{} did not reproduce", self.doc.edges[edge].id);
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn all(&mut self) -> Result<(), Halt> {
        for position in 0..self.doc.edges.len() {
            self.check_limits()?;
            let edge = &self.doc.edges[position];
            if edge.status != Status::Ok {
                self.push(Step {
                    edge: position,
                    outcome: Outcome::RecordedFailed,
                    actual: None,
                    note: Some(format!("it was {} in the walk", edge.status.as_str())),
                });
                continue;
            }
            let Some(view) = self.stand_on(edge.from)? else {
                self.push(not_run(position, &format!("s{} could not be opened again", edge.from)));
                continue;
            };
            let from = edge.from;
            let done = self.perform(position, &view)?;
            // The page is still the screen it was when nothing on it changed: the next edge of that screen can use it.
            self.at = match (&done.step.outcome, done.view) {
                (Outcome::Reproduced, Some(next)) if !done.changed => Some((from, next)),
                _ => None,
            };
            self.push(done.step);
        }
        Ok(())
    }

    /// Opens the start page and checks it is `s0`. The inner error is what to tell when it is not.
    fn open_start(&mut self) -> Result<Result<View, String>, Halt> {
        self.check_limits()?;
        match self.browser.load(&self.doc.start_url) {
            Ok(view) => {
                let look = look_of(&view);
                if self.screens.matches(self.screens.all()[0].id, &look) {
                    Ok(Ok(view))
                } else {
                    Ok(Err(format!("the start page is not s0 (it is {})", self.name_of(&look))))
                }
            }
            Err(StepError::Cancelled) => Err(Halt::Stop(Ended::Cancelled)),
            Err(StepError::Fatal(message)) => Err(Halt::Error(message)),
            Err(other) => Ok(Err(format!(
                "the start page did not open: {}",
                redact_text(&other.to_string())
            ))),
        }
    }

    /// Makes sure the browser is on `screen`, as the walk read it. `Ok(None)` when it cannot be.
    ///
    /// The screen's address is opened first, twice at most: a page can look a little different from one load to the
    /// next. A screen that its address does not bring back is then reopened by performing the shortest recorded way to
    /// it (twice at most), and remembered, so that the address is not tried again for it.
    fn stand_on(&mut self, screen: usize) -> Result<Option<View>, Halt> {
        if let Some((at, view)) = &self.at {
            if *at == screen {
                return Ok(Some(view.clone()));
            }
        }
        self.at = None;
        if self.unreachable.contains(&screen) {
            return Ok(None);
        }
        // An address with hidden parameters cannot be opened.
        let state = &self.doc.states[screen];
        if !self.needs_way.contains(&screen) && !state.url.contains(crate::url_safety::REDACTED) {
            let address = state.url.clone();
            for _attempt in 0..2 {
                self.check_limits()?;
                let view = match self.browser.load(&address) {
                    Ok(view) => view,
                    Err(StepError::Cancelled) => return Err(Halt::Stop(Ended::Cancelled)),
                    Err(StepError::Fatal(message)) => return Err(Halt::Error(message)),
                    Err(_) => continue,
                };
                if self.screens.matches(self.screens.all()[screen].id, &look_of(&view)) {
                    return Ok(Some(view));
                }
            }
            self.needs_way.insert(screen);
        }
        let way = if screen == 0 {
            Some(Vec::new())
        } else {
            shortest_way(self.doc, screen)
        };
        let Some(way) = way else {
            self.say(&format!(
                "s{screen} cannot be opened again: the graph has no way to it that is safe to repeat"
            ));
            self.unreachable.insert(screen);
            return Ok(None);
        };
        for _attempt in 0..2 {
            let before = self.actions;
            let result = self.follow(screen, &way)?;
            (self.progress)(&ReplayEvent::Reopen {
                state: &format!("s{screen}"),
                actions: (self.actions - before) as usize,
                reopened: result.is_some(),
            });
            if result.is_some() {
                return Ok(result);
            }
        }
        self.say(&format!(
            "s{screen} cannot be opened again: performing the way to it did not bring it back"
        ));
        self.unreachable.insert(screen);
        Ok(None)
    }

    /// Opens the start page and performs `way` from it. `None` when the page or any step is not what the graph says.
    fn follow(&mut self, screen: usize, way: &[usize]) -> Result<Option<View>, Halt> {
        let Ok(mut view) = self.open_start()? else {
            return Ok(None);
        };
        for &edge in way {
            self.check_limits()?;
            let done = self.perform(edge, &view)?;
            match done.view {
                Some(next) if done.step.outcome == Outcome::Reproduced => view = next,
                _ => return Ok(None),
            }
        }
        debug_assert!(way.last().is_none_or(|&last| self.doc.edges[last].to == Some(screen)));
        Ok(Some(view))
    }

    /// Does what edge `position` did, on `view`, and says whether the page is now the screen it led to.
    fn perform(&mut self, position: usize, view: &View) -> Result<Done, Halt> {
        let edge = &self.doc.edges[position];
        let nothing = |outcome, note: String| Done {
            step: Step {
                edge: position,
                outcome,
                actual: None,
                note: Some(note),
            },
            view: None,
            changed: false,
        };
        let Some((component, instance)) = actionable(view, &edge.component) else {
            return Ok(nothing(
                Outcome::Missing,
                "the component is not on the page, or cannot be acted on (hidden or disabled)".to_string(),
            ));
        };
        let Some(kind) = kind_of(&component.role) else {
            return Ok(nothing(
                Outcome::Blocked,
                format!("a {} is not something a replay acts on", component.role),
            ));
        };
        let decision = self.options.rules.decide(&Candidate {
            role: &component.role,
            name: &component.name,
            href: instance.href.as_deref(),
            expanded: instance.expanded,
            page_url: &view.url,
        });
        if let Decision::Skip(skip) = decision {
            return Ok(nothing(
                Outcome::Blocked,
                format!("left alone by the rules: {}", skip.reason()),
            ));
        }
        let act = match (edge.action, kind) {
            (ActionKind::Type, _) => match &edge.value {
                Some(text) => Act::Type(text.clone()),
                None => {
                    return Ok(nothing(
                        Outcome::Blocked,
                        "the graph has no value for what was typed (it was written by an older traceqa)".to_string(),
                    ))
                }
            },
            _ => Act::Click,
        };
        let key = key_for(&component.role);
        let timeout_ms = self.options.action_timeout.as_millis() as u64;
        let result = self.act_with_key(instance, &act, kind, key, timeout_ms)?;
        self.actions += 1;
        let acted = match result {
            Ok(acted) => acted,
            Err(StepError::NotFound(why)) => {
                self.actions -= 1;
                return Ok(nothing(Outcome::Missing, redact_text(&why)));
            }
            Err(error) => {
                return Ok(nothing(
                    Outcome::Diverged,
                    format!("the action did not complete: {}", redact_text(&error.to_string())),
                ))
            }
        };
        Ok(self.compare(position, acted))
    }

    /// Is the page after the action the screen the edge led to?
    fn compare(&self, position: usize, acted: Acted) -> Done {
        let Acted { effect, view } = acted;
        let look = look_of(&view);
        let edge = &self.doc.edges[position];
        let to = edge.to.unwrap_or(edge.from);
        let reproduced = self.screens.matches(self.screens.all()[to].id, &look);
        let (outcome, actual, note) = if reproduced {
            (Outcome::Reproduced, Some(format!("s{to}")), None)
        } else {
            let actual = self.name_of(&look);
            let note = format!("the page is {actual}, and the graph says s{to}");
            (Outcome::Diverged, Some(actual), Some(note))
        };
        Done {
            step: Step {
                edge: position,
                outcome,
                actual,
                note,
            },
            view: Some(view),
            changed: effect.changed,
        }
    }

    /// Does `act` on `instance`. A click that fails or times out is tried once with the key that does the same, as the
    /// walk does: a link under an overlay cannot be clicked and can be pressed.
    fn act_with_key(
        &mut self,
        instance: &ViewInstance,
        act: &Act,
        kind: Kind,
        key: &'static str,
        timeout_ms: u64,
    ) -> Result<Result<Acted, StepError>, Halt> {
        let mut result = self.browser.act(instance, act, timeout_ms);
        let blocked = matches!(result, Err(StepError::Failed(_) | StepError::Timeout(_)));
        if blocked && kind == Kind::Click {
            self.check_limits()?;
            let retried = self.browser.act(instance, &Act::Press(key), PRESS_TIMEOUT_MS);
            match retried {
                Ok(_) => result = retried,
                Err(StepError::Cancelled | StepError::Fatal(_)) => result = retried,
                Err(_) => {}
            }
        }
        match result {
            Err(error @ (StepError::Cancelled | StepError::Fatal(_))) => Err(halt(error)),
            other => Ok(other),
        }
    }

    /// Which screen of the graph a page is: `sN`, or `unknown` when it is none of them or has nothing on it.
    fn name_of(&self, look: &Look) -> String {
        let mut trial = self.screens.clone();
        match trial.observe(look) {
            Ok(seen) if !seen.is_new => seen.id.to_string(),
            _ => "unknown".to_string(),
        }
    }
}

/// What a page looks like to the matcher. The address goes through [`redact`] first, because the graph holds addresses
/// with secret parameters hidden and a page is compared with those.
fn look_of(view: &View) -> Look {
    let mut look = Look::new(&redact(&view.url), visible_ids(view), view.modal.as_deref());
    look.frames = view.frames.iter().cloned().collect();
    look
}
