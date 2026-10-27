//! The walk itself: a worklist of (screen, component) pairs, taken in a fixed order, each tried once.
//!
//! Plain data in, plain data out. It talks to a [`Browser`], asks [`Rules`](crate::policy::Rules) before every action,
//! and gives screens their names through [`Screens`]. Nothing here reads a clock except for the wall-clock budget,
//! and nothing random is used, so a site that answers the same way gives the same graph.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use url::Url;

use super::graph::{Edge, Graph, Settings, Skipped, State, Status, Stop};
use super::model::{Act, Acted, Browser, StepError, View, ViewComponent, ViewInstance};
use super::{data, Options, Strategy, MAX_TIMEOUTS_IN_A_ROW};
use crate::policy::{Candidate, Decision};
use crate::state::{chrome_ids, clean_url, Look, Screens, DEFAULT_CHROME_SHARE};
use crate::url_safety::redact;

/// The result of a walk. A walk that failed in the middle still has the graph of what it found.
pub struct Explored {
    pub graph: Graph,
    /// Why the browser failed, when `graph.stopped` is [`Stop::Error`].
    pub error: Option<String>,
}

/// A line for the person watching.
pub type Progress<'a> = &'a mut dyn FnMut(&str);

/// The time a keyboard retry of a blocked click is given, at most.
const PRESS_TIMEOUT_MS: u64 = 2_000;

/// What the walker does with a role.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Click,
    Type,
    /// A list to pick from. The options are not known, so these are left alone.
    Select,
}

fn kind_of(role: &str) -> Option<Kind> {
    match role {
        "link" | "button" | "tab" | "menuitem" | "checkbox" | "radio" | "switch" | "disclosure" => Some(Kind::Click),
        "textbox" | "searchbox" => Some(Kind::Type),
        "combobox" | "listbox" => Some(Kind::Select),
        _ => None,
    }
}

/// The key that does what a click does, for the roles that take one.
fn key_for(role: &str) -> &'static str {
    match role {
        "checkbox" | "radio" | "switch" => "Space",
        _ => "Enter",
    }
}

/// What the walker knows about a screen besides what [`Screens`] keeps.
struct Info {
    depth: u32,
    /// The address as the browser reported it the first time, to open the screen again. Never written to a file.
    open_at: String,
    /// `role "name"` of every candidate, for the file.
    labels: BTreeMap<String, String>,
}

/// How a step ended, when it did not give a page.
enum Halt {
    Stop(Stop),
    /// The browser failed. The message is for the run's events.
    Error(String),
}

/// A step that gave no page, as the reason the whole walk ends. Only for steps nothing can be done without.
fn halt(error: StepError) -> Halt {
    match error {
        StepError::Cancelled => Halt::Stop(Stop::Cancelled),
        other => Halt::Error(other.to_string()),
    }
}

struct Walker<'a> {
    browser: &'a mut dyn Browser,
    options: &'a Options,
    cancel: &'a AtomicBool,
    progress: Progress<'a>,
    started: Instant,
    screens: Screens,
    infos: Vec<Info>,
    worklist: VecDeque<(usize, String)>,
    edges: Vec<Edge>,
    skipped: Vec<Skipped>,
    skipped_seen: BTreeSet<(usize, String)>,
    unreachable: BTreeSet<usize>,
    /// Screens the address does not bring back (a dialog, an open drawer): they are reopened by replaying the actions
    /// that led to them, without trying the address first.
    needs_replay: BTreeSet<usize>,
    actions: u32,
    timeouts_in_a_row: u32,
    /// Some screens were not explored because they are too deep.
    depth_cut: bool,
    /// The screen the browser is standing on exactly as it was first read, and that reading.
    at: Option<(usize, View)>,
    chrome: BTreeSet<String>,
    /// Page-wide components already queued or skipped, so each is dealt with on the first screen that has it.
    chrome_seen: BTreeSet<String>,
    chrome_pages: u32,
}

pub fn explore(browser: &mut dyn Browser, options: &Options, cancel: &AtomicBool, progress: Progress) -> Explored {
    let mut walker = Walker {
        browser,
        options,
        cancel,
        progress,
        started: Instant::now(),
        screens: Screens::new(options.similarity, BTreeSet::new()),
        infos: Vec::new(),
        worklist: VecDeque::new(),
        edges: Vec::new(),
        skipped: Vec::new(),
        skipped_seen: BTreeSet::new(),
        unreachable: BTreeSet::new(),
        needs_replay: BTreeSet::new(),
        actions: 0,
        timeouts_in_a_row: 0,
        depth_cut: false,
        at: None,
        chrome: BTreeSet::new(),
        chrome_seen: BTreeSet::new(),
        chrome_pages: 0,
    };
    let (stopped, error) = match walker.run() {
        Ok(()) => (
            if walker.depth_cut {
                Stop::MaxDepth
            } else {
                Stop::Complete
            },
            None,
        ),
        Err(Halt::Stop(stop)) => (stop, None),
        Err(Halt::Error(message)) => (Stop::Error, Some(message)),
    };
    Explored {
        graph: walker.into_graph(stopped),
        error,
    }
}

impl Walker<'_> {
    fn say(&mut self, line: &str) {
        (self.progress)(line);
    }

    fn run(&mut self) -> Result<(), Halt> {
        self.check_limits()?;
        let first = self.browser.load(&self.options.start_url).map_err(halt)?;
        let first_ids = visible_ids(&first);
        let mut pages = vec![first_ids];
        // Any load, even one that failed, takes the browser away from the start page: a failed load closes the
        // session that was open.
        let mut moved = false;
        for address in self.chrome_candidates(&first) {
            self.check_limits()?;
            moved = true;
            match self.browser.load(&address) {
                Ok(view) => pages.push(visible_ids(&view)),
                Err(error @ (StepError::Cancelled | StepError::Fatal(_))) => return Err(halt(error)),
                // A page that will not open only tells less about the header.
                Err(_) => {}
            }
        }
        self.chrome_pages = pages.len() as u32;
        self.chrome = chrome_ids(&pages, DEFAULT_CHROME_SHARE);
        self.screens = Screens::new(self.options.similarity, self.chrome.clone());

        // The start page is read again only if the browser has been somewhere else since.
        let start = if moved {
            self.browser.load(&self.options.start_url).map_err(halt)?
        } else {
            first
        };
        let seen = self
            .screens
            .observe(&look_of(&start))
            .map_err(|error| Halt::Error(format!("the start page cannot be used: {error}")))?;
        let root = seen.id.index();
        self.add_screen(root, 0, &start);
        self.at = Some((root, start));

        loop {
            self.check_limits()?;
            let Some((screen, component)) = self.take() else {
                return Ok(());
            };
            if self.unreachable.contains(&screen) {
                continue;
            }
            self.step(screen, &component)?;
        }
    }

    /// The things that end a walk wherever it is: cancel, the clock and the number of requests.
    fn check_limits(&self) -> Result<(), Halt> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(Halt::Stop(Stop::Cancelled));
        }
        if self.started.elapsed() >= self.options.budgets.timeout {
            return Err(Halt::Stop(Stop::Timeout));
        }
        if let Some(counts) = self.browser.requests() {
            if counts
                .values()
                .any(|&n| n >= self.options.budgets.max_requests_per_origin)
            {
                return Err(Halt::Stop(Stop::MaxRequests));
            }
        }
        Ok(())
    }

    fn take(&mut self) -> Option<(usize, String)> {
        match self.options.strategy {
            Strategy::Bfs => self.worklist.pop_front(),
            Strategy::Dfs => self.worklist.pop_back(),
        }
    }

    /// Pages to read before the walk starts, besides the first: the links on it that the rules allow, in the order
    /// the walk would take them.
    fn chrome_candidates(&self, first: &View) -> Vec<String> {
        let Ok(base) = Url::parse(&first.url) else {
            return Vec::new();
        };
        let own = clean_url(&first.url).ok();
        let mut found = Vec::new();
        let mut seen = BTreeSet::new();
        for component in ordered(first) {
            if found.len() >= self.options.chrome_pages {
                break;
            }
            if component.role != "link" {
                continue;
            }
            let Some(instance) = component.instances.iter().find(|i| i.is_actionable()) else {
                continue;
            };
            let Some(href) = instance.href.as_deref() else {
                continue;
            };
            if self.decide(component, instance, &first.url) != Decision::Allow {
                continue;
            }
            let Ok(target) = base.join(href) else {
                continue;
            };
            let Ok(cleaned) = clean_url(target.as_str()) else {
                continue;
            };
            if Some(&cleaned) == own.as_ref() || !seen.insert(cleaned) {
                continue;
            }
            found.push(target.to_string());
        }
        found
    }

    fn decide(&self, component: &ViewComponent, instance: &ViewInstance, page_url: &str) -> Decision {
        self.options.rules.decide(&Candidate {
            role: &component.role,
            name: &component.name,
            href: instance.href.as_deref(),
            expanded: instance.expanded,
            page_url,
        })
    }

    /// Records a screen the first time it is seen and queues what can be done on it.
    fn add_screen(&mut self, index: usize, depth: u32, view: &View) {
        debug_assert_eq!(index, self.infos.len());
        let mut labels = BTreeMap::new();
        let mut queue = Vec::new();
        for component in ordered(view) {
            let Some(kind) = kind_of(&component.role) else {
                continue;
            };
            let Some(instance) = component.instances.iter().find(|i| i.is_actionable()) else {
                continue;
            };
            // The header and the footer are the same on every screen: they are dealt with on the first screen that
            // shows them, and not again on each of the others.
            let page_wide = self.chrome.contains(&component.id);
            if page_wide && self.chrome_seen.contains(&component.id) {
                continue;
            }
            let label = label_of(component);
            if let Some(reason) = self.reason_to_skip(component, instance, &view.url, kind) {
                self.note_skip(index, component, &label, reason);
                if page_wide {
                    self.chrome_seen.insert(component.id.clone());
                }
                continue;
            }
            // On a screen too deep to be explored nothing is queued, so the link has not been dealt with.
            if page_wide && depth < self.options.budgets.max_depth {
                self.chrome_seen.insert(component.id.clone());
            }
            labels.insert(component.id.clone(), label);
            queue.push((index, component.id.clone()));
        }
        self.infos.push(Info {
            depth,
            open_at: view.url.clone(),
            labels,
        });
        if depth >= self.options.budgets.max_depth {
            self.depth_cut |= !queue.is_empty();
            return;
        }
        match self.options.strategy {
            Strategy::Bfs => self.worklist.extend(queue),
            Strategy::Dfs => self.worklist.extend(queue.into_iter().rev()),
        }
    }

    fn reason_to_skip(&self, c: &ViewComponent, i: &ViewInstance, page_url: &str, kind: Kind) -> Option<String> {
        if let Decision::Skip(skip) = self.decide(c, i, page_url) {
            return Some(skip.reason().to_string());
        }
        (kind == Kind::Select).then(|| "no-options".to_string())
    }

    fn note_skip(&mut self, screen: usize, component: &ViewComponent, label: &str, reason: String) {
        if self.skipped_seen.insert((screen, component.id.clone())) {
            self.skipped.push(Skipped {
                state: format!("s{screen}"),
                component: component.id.clone(),
                label: label.to_string(),
                reason,
            });
        }
    }

    /// Makes sure the browser is on `screen` as it was first read. `Ok(None)` when it cannot be, and the screen is
    /// then given up.
    ///
    /// First the screen's address is opened. A page can look a little different from one load to the next, so a load
    /// that is not the screen is tried once more. A screen that its address does not bring back (a dialog, an open
    /// drawer) fails both times; it is then remembered, and from then on it is reopened by replaying the shortest
    /// chain of actions that led to it, which costs actions from the budget. A replay that does not end on the screen
    /// is tried once more and then the screen is given up. Nothing is tried a third time.
    fn stand_on(&mut self, screen: usize) -> Result<Option<View>, Halt> {
        if let Some((at, view)) = &self.at {
            if *at == screen {
                return Ok(Some(view.clone()));
            }
        }
        self.at = None;
        if !self.needs_replay.contains(&screen) {
            let address = self.infos[screen].open_at.clone();
            for _attempt in 0..2 {
                self.check_limits()?;
                let view = match self.browser.load(&address) {
                    Ok(view) => view,
                    Err(StepError::Cancelled) => return Err(Halt::Stop(Stop::Cancelled)),
                    Err(StepError::Fatal(message)) => return Err(Halt::Error(message)),
                    Err(_) => continue,
                };
                // Is it that screen by the rule? Not whether that screen is the closest of all: two screens at one
                // address that are not alike can both be within the share of a page opened again.
                if self.screens.matches(self.screens.all()[screen].id, &look_of(&view)) {
                    self.at = Some((screen, view.clone()));
                    return Ok(Some(view));
                }
            }
            self.needs_replay.insert(screen);
        }
        // The start page is the screen its own address opens, so there is nothing to replay for it.
        let Some(path) = (screen != 0).then(|| self.path_to(screen)).flatten() else {
            return Ok(self.give_up(screen, "no recorded way to reach it"));
        };
        for _attempt in 0..2 {
            if let Some(view) = self.replay(screen, &path)? {
                self.say(&format!("s{screen} reopened by replaying {} actions", path.len()));
                self.at = Some((screen, view.clone()));
                return Ok(Some(view));
            }
        }
        Ok(self.give_up(screen, "replaying the actions that led to it did not bring it back"))
    }

    fn give_up(&mut self, screen: usize, why: &str) -> Option<View> {
        self.unreachable.insert(screen);
        self.say(&format!(
            "s{screen} cannot be opened again ({why}), so what is left on it is not tried"
        ));
        None
    }

    /// The shortest chain of recorded edges from the start screen to `screen`, as (component, screen it led to).
    /// Edges that typed text are left out: doing a write again could do its harm again.
    fn path_to(&self, screen: usize) -> Option<Vec<(String, usize)>> {
        let index = |name: &str| name.strip_prefix('s').and_then(|n| n.parse::<usize>().ok());
        // How each screen was first reached from screen 0, as the edge that did it.
        let mut via: BTreeMap<usize, (usize, &Edge)> = BTreeMap::new();
        let mut queue = VecDeque::from([0usize]);
        while let Some(here) = queue.pop_front() {
            if here == screen {
                break;
            }
            for edge in &self.edges {
                if edge.status != Status::Ok || edge.action == "type" || index(&edge.from) != Some(here) {
                    continue;
                }
                let Some(to) = edge.to.as_deref().and_then(index) else {
                    continue;
                };
                if to != 0 && to != here && !via.contains_key(&to) {
                    via.insert(to, (here, edge));
                    queue.push_back(to);
                }
            }
        }
        let mut path = Vec::new();
        let mut at = screen;
        while at != 0 {
            let (from, edge) = via.get(&at)?;
            path.push((edge.component.clone(), at));
            at = *from;
        }
        path.reverse();
        Some(path)
    }

    /// Opens the start page and does `path` on it, checking after every action that the page is the screen the edge
    /// led to. `Ok(None)` at the first thing that is not as it was recorded. Every action is asked of the rules again
    /// and counts against the budget.
    fn replay(&mut self, screen: usize, path: &[(String, usize)]) -> Result<Option<View>, Halt> {
        self.check_limits()?;
        let start = self.infos[0].open_at.clone();
        let mut view = match self.browser.load(&start) {
            Ok(view) => view,
            Err(StepError::Cancelled) => return Err(Halt::Stop(Stop::Cancelled)),
            Err(StepError::Fatal(message)) => return Err(Halt::Error(message)),
            Err(_) => return Ok(None),
        };
        if !self.screens.matches(self.screens.all()[0].id, &look_of(&view)) {
            return Ok(None);
        }
        let timeout_ms = self.options.budgets.action_timeout.as_millis() as u64;
        for (component, to) in path {
            self.check_limits()?;
            let found = view
                .components
                .iter()
                .find(|c| &c.id == component)
                .and_then(|c| c.instances.iter().find(|i| i.is_actionable()).map(|i| (c, i)));
            let Some((comp, instance)) = found else {
                return Ok(None);
            };
            let Some(kind) = kind_of(&comp.role) else {
                return Ok(None);
            };
            if self.reason_to_skip(comp, instance, &view.url, kind).is_some() {
                return Ok(None);
            }
            if self.actions >= self.options.budgets.max_actions {
                return Err(Halt::Stop(Stop::MaxActions));
            }
            let key = key_for(&comp.role);
            let (_, result) = self.act_with_key_fallback(instance, &Act::Click, kind, key, timeout_ms)?;
            self.actions += 1;
            view = match result {
                Ok(acted) => acted.view,
                Err(StepError::Cancelled) => return Err(Halt::Stop(Stop::Cancelled)),
                Err(StepError::Fatal(message)) => return Err(Halt::Error(message)),
                Err(_) => return Ok(None),
            };
            if !self.screens.matches(self.screens.all()[*to].id, &look_of(&view)) {
                return Ok(None);
            }
        }
        debug_assert!(path.last().is_none_or(|(_, last)| *last == screen));
        Ok(Some(view))
    }

    /// Does `act` on `instance`. A click that fails or times out is tried once with the key that does the same. The
    /// word is the action that gave the result, for the graph.
    fn act_with_key_fallback(
        &mut self,
        instance: &ViewInstance,
        act: &Act,
        kind: Kind,
        key: &'static str,
        timeout_ms: u64,
    ) -> Result<(&'static str, Result<Acted, StepError>), Halt> {
        let mut name = act.name();
        let mut result = self.browser.act(instance, act, timeout_ms);
        let blocked = matches!(result, Err(StepError::Failed(_) | StepError::Timeout(_)));
        if blocked && kind == Kind::Click {
            self.check_limits()?;
            let retried = self
                .browser
                .act(instance, &Act::Press(key), timeout_ms.min(PRESS_TIMEOUT_MS));
            match retried {
                Ok(_) => {
                    name = "press";
                    result = retried;
                }
                // The browser or the run is gone: that is what to tell.
                Err(StepError::Cancelled | StepError::Fatal(_)) => result = retried,
                // Otherwise the click is what failed. A key press that finds the page gone (the click started a
                // navigation that never finished) says "not found", which would hide the timeout.
                Err(_) => {}
            }
        }
        Ok((name, result))
    }

    /// Tries one component on one screen.
    fn step(&mut self, screen: usize, component: &str) -> Result<(), Halt> {
        let Some(view) = self.stand_on(screen)? else {
            return Ok(());
        };
        let label = self.infos[screen].labels.get(component).cloned().unwrap_or_default();
        let found = view
            .components
            .iter()
            .find(|c| c.id == component)
            .and_then(|c| c.instances.iter().find(|i| i.is_actionable()).map(|i| (c, i)));
        let Some((comp, instance)) = found else {
            self.record(screen, component, &label, "click", &Outcome::none(Status::Missing));
            return Ok(());
        };
        let Some(kind) = kind_of(&comp.role) else {
            return Ok(());
        };
        // The element in front of the browser is the one that counts, not the one the screen was first read with.
        if let Some(reason) = self.reason_to_skip(comp, instance, &view.url, kind) {
            self.note_skip(screen, comp, &label, reason);
            return Ok(());
        }
        if self.actions >= self.options.budgets.max_actions {
            return Err(Halt::Stop(Stop::MaxActions));
        }

        let timeout_ms = self.options.budgets.action_timeout.as_millis() as u64;
        let act = match kind {
            Kind::Type => Act::Type(data::value_for(self.options.seed, component, &comp.name)),
            _ => Act::Click,
        };
        let key = key_for(&comp.role);
        let (name, result) = self.act_with_key_fallback(instance, &act, kind, key, timeout_ms)?;
        self.at = None;

        let acted = match result {
            Ok(acted) => acted,
            Err(StepError::Cancelled) => return Err(Halt::Stop(Stop::Cancelled)),
            Err(StepError::Fatal(message)) => return Err(Halt::Error(message)),
            Err(StepError::NotFound(why)) => {
                self.record(
                    screen,
                    component,
                    &label,
                    name,
                    &Outcome::because(Status::Missing, &why),
                );
                return Ok(());
            }
            Err(StepError::Timeout(why)) => {
                self.actions += 1;
                self.timeouts_in_a_row += 1;
                self.record(
                    screen,
                    component,
                    &label,
                    name,
                    &Outcome::because(Status::Timeout, &why),
                );
                if self.timeouts_in_a_row >= MAX_TIMEOUTS_IN_A_ROW {
                    return Err(Halt::Stop(Stop::ActionTimeout));
                }
                return Ok(());
            }
            Err(StepError::Failed(why)) => {
                self.actions += 1;
                self.timeouts_in_a_row = 0;
                self.record(
                    screen,
                    component,
                    &label,
                    name,
                    &Outcome::because(Status::Failed, &why.to_string()),
                );
                return Ok(());
            }
        };
        self.timeouts_in_a_row = 0;
        self.arrived(screen, component, &label, name, acted)
    }

    /// The action worked and the page was read again: which screen is it?
    fn arrived(
        &mut self,
        from: usize,
        component: &str,
        label: &str,
        name: &'static str,
        acted: Acted,
    ) -> Result<(), Halt> {
        let Acted { effect, view } = acted;
        let mut trial = self.screens.clone();
        let seen = match trial.observe(&look_of(&view)) {
            Ok(seen) => seen,
            Err(why) => {
                // A page with nothing on it, or an address that cannot be read, is a failed read and not a screen.
                self.actions += 1;
                self.record(
                    from,
                    component,
                    label,
                    name,
                    &Outcome::because(Status::Failed, &why.to_string()),
                );
                return Ok(());
            }
        };
        if seen.is_new && trial.len() > self.options.budgets.max_states as usize {
            return Err(Halt::Stop(Stop::MaxStates));
        }
        self.actions += 1;
        self.screens = trial;
        let to = seen.id.index();
        if seen.is_new {
            let depth = self.infos[from].depth + 1;
            self.add_screen(to, depth, &view);
            let url = self.screens.get(seen.id).map(|s| redact(&s.url)).unwrap_or_default();
            self.say(&format!("s{to} is new (depth {depth}): {url}"));
            self.at = Some((to, view));
        } else if !effect.changed {
            // Nothing happened, so the page is still as it was first read.
            self.at = Some((from, view));
        }
        self.record(
            from,
            component,
            label,
            name,
            &Outcome {
                note: None,
                to: Some(to),
                status: Status::Ok,
                navigated: effect.navigated,
                popups: effect.popups,
                dialogs: effect.dialogs,
                downloads: effect.downloads,
            },
        );
        Ok(())
    }

    fn record(&mut self, from: usize, component: &str, label: &str, action: &'static str, outcome: &Outcome) {
        let id = format!("e{}", self.edges.len());
        let end = outcome.to.map_or_else(
            || match &outcome.note {
                Some(note) => format!("{} ({note})", outcome.status.as_str()),
                None => outcome.status.as_str().to_string(),
            },
            |to| format!("s{to}"),
        );
        self.say(&format!("{id}: {action} {label} on s{from} -> {end}"));
        self.edges.push(Edge {
            id,
            from: format!("s{from}"),
            component: component.to_string(),
            label: label.to_string(),
            action,
            to: outcome.to.map(|to| format!("s{to}")),
            status: outcome.status,
            navigated: outcome.navigated,
            popups: outcome.popups,
            dialogs: outcome.dialogs,
            downloads: outcome.downloads,
        });
    }

    fn into_graph(self, stopped: Stop) -> Graph {
        let options = self.options;
        let states = self
            .screens
            .all()
            .iter()
            .zip(&self.infos)
            .map(|(screen, info)| State {
                id: screen.id.to_string(),
                url: redact(&screen.url),
                modal: screen.modal.clone(),
                components: screen.ids.iter().cloned().collect(),
                frames: screen.frames.iter().cloned().collect(),
                depth: info.depth,
            })
            .collect();
        Graph {
            settings: Settings {
                start_url: redact(&options.start_url),
                seed: options.seed,
                strategy: options.strategy.as_str(),
                similarity_per_mille: options.similarity.per_mille(),
                max_states: options.budgets.max_states,
                max_actions: options.budgets.max_actions,
                max_depth: options.budgets.max_depth,
                action_timeout_ms: options.budgets.action_timeout.as_millis() as u64,
                timeout_ms: options.budgets.timeout.as_millis() as u64,
                max_requests_per_origin: options.budgets.max_requests_per_origin,
            },
            chrome_pages: self.chrome_pages,
            chrome: self.chrome.into_iter().collect(),
            stopped,
            actions: self.actions,
            states,
            edges: self.edges,
            skipped: self.skipped,
            unreachable: self
                .unreachable
                .into_iter()
                .map(|screen| format!("s{screen}"))
                .collect(),
        }
    }
}

struct Outcome {
    /// Why it did not work, for the progress line. Not written to the graph.
    note: Option<String>,
    to: Option<usize>,
    status: Status,
    navigated: bool,
    popups: u32,
    dialogs: u32,
    downloads: u32,
}

impl Outcome {
    fn because(status: Status, why: &str) -> Self {
        Self {
            note: Some(why.to_string()),
            ..Self::none(status)
        }
    }

    fn none(status: Status) -> Self {
        Self {
            note: None,
            to: None,
            status,
            navigated: false,
            popups: 0,
            dialogs: 0,
            downloads: 0,
        }
    }
}

/// The ids of the components that have a visible instance.
fn visible_ids(view: &View) -> BTreeSet<String> {
    view.components
        .iter()
        .filter(|c| c.instances.iter().any(|i| i.visible))
        .map(|c| c.id.clone())
        .collect()
}

fn look_of(view: &View) -> Look {
    let mut look = Look::new(&view.url, visible_ids(view), view.modal.as_deref());
    look.frames = view.frames.iter().cloned().collect();
    look
}

/// The components in the order the walk takes them: by path, then id. It depends on the page and not on the order the
/// collector listed them in.
fn ordered(view: &View) -> Vec<&ViewComponent> {
    let mut all: Vec<&ViewComponent> = view.components.iter().collect();
    all.sort_by(|a, b| (&a.path, &a.id).cmp(&(&b.path, &b.id)));
    all
}

/// `role "name"`, on one line and not long.
fn label_of(component: &ViewComponent) -> String {
    let name: String = component.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(60).collect();
    format!("{} \"{}\"", component.role, name)
}
