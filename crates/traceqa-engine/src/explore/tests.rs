//! The rules of the walk, against a small site that lives in memory. Whether the real browser behaves is not tested
//! here: `tests/test_explore.py` runs the real command in Chromium, Firefox and WebKit.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::values::FAILURES_BEFORE_OFF;
use super::*;
use crate::policy::Rules;
use crate::state::{Similarity, DEFAULT_SIMILARITY};

const ORIGIN: &str = "http://shop.test";

// ---------------------------------------------------------------------------------------------------------------
// A site in memory
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct C {
    id: String,
    role: &'static str,
    name: String,
    path: String,
    href: Option<String>,
    expanded: Option<bool>,
    to: Option<&'static str>,
    visible: bool,
    enabled: bool,
    /// What an action on it does instead of working.
    fails: Option<StepError>,
    /// Fails when clicked, works when a key is pressed.
    click_blocked: bool,
    /// What a key press does instead of working (the page has moved on, say).
    press_fails: Option<StepError>,
    hints: Hints,
}

fn c(name: &str, role: &'static str) -> C {
    C {
        id: format!("c_{}", name.to_lowercase().replace(' ', "_")),
        role,
        name: name.to_string(),
        path: "main".to_string(),
        href: None,
        expanded: None,
        to: None,
        visible: true,
        enabled: true,
        fails: None,
        click_blocked: false,
        press_fails: None,
        hints: Hints::default(),
    }
}

impl C {
    fn link(name: &str, to: &'static str, href: &str) -> Self {
        Self {
            to: Some(to),
            href: Some(href.to_string()),
            ..c(name, "link")
        }
    }
    fn button(name: &str, to: &'static str) -> Self {
        Self {
            to: Some(to),
            ..c(name, "button")
        }
    }
    fn at(mut self, path: &str) -> Self {
        self.path = path.to_string();
        self
    }
    fn fails(mut self, error: StepError) -> Self {
        self.fails = Some(error);
        self
    }
    fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }
    fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
    fn input_type(mut self, kind: &str) -> Self {
        self.hints.input_type = Some(kind.to_string());
        self
    }
    fn expanded(mut self) -> Self {
        self.expanded = Some(false);
        self
    }
}

#[derive(Clone)]
struct P {
    url: String,
    modal: Option<String>,
    comps: Vec<C>,
}

fn page(path: &str, comps: Vec<C>) -> P {
    P {
        url: format!("{ORIGIN}{path}"),
        modal: None,
        comps,
    }
}

/// A page that is a leaf: one thing to look at and nothing to act on.
fn leaf(path: &str, title: &str) -> P {
    page(path, vec![c(title, "heading")])
}

type Pages = BTreeMap<&'static str, P>;

struct Fake {
    pages: Pages,
    current: &'static str,
    /// Every action as `component id`, in order.
    acted: Vec<String>,
    /// Every action as `kind component id`.
    kinds: Vec<String>,
    /// Every key pressed.
    keys: Vec<String>,
    /// Every text typed.
    typed: Vec<String>,
    /// A load that failed closed the session that was open, as the real browser does. Nothing can be acted on until
    /// a load works.
    stranded: bool,
    /// Every address opened.
    loaded: Vec<String>,
    requests: u64,
    per_request: u64,
    count_requests: bool,
    delay: Duration,
    cancel: Option<(usize, &'static AtomicBool)>,
    reverse: bool,
    /// How long a load takes, as on a slow site.
    load_delay: Duration,
    /// The browser refuses this request when the component is acted on: (component id, method, address).
    refuse: Option<(&'static str, &'static str, &'static str)>,
}

impl Fake {
    fn new(pages: Vec<(&'static str, P)>) -> Self {
        Self {
            pages: pages.into_iter().collect(),
            current: "home",
            acted: Vec::new(),
            kinds: Vec::new(),
            keys: Vec::new(),
            typed: Vec::new(),
            stranded: false,
            loaded: Vec::new(),
            requests: 0,
            per_request: 1,
            count_requests: true,
            delay: Duration::ZERO,
            cancel: None,
            reverse: false,
            load_delay: Duration::ZERO,
            refuse: None,
        }
    }

    fn view(&self) -> View {
        let page = &self.pages[self.current];
        let mut comps: Vec<ViewComponent> = page
            .comps
            .iter()
            .map(|c| ViewComponent {
                id: c.id.clone(),
                role: c.role.to_string(),
                name: c.name.clone(),
                path: c.path.clone(),
                instances: vec![ViewInstance {
                    selector: format!("sel:{}", c.id),
                    frame_id: "main".to_string(),
                    visible: c.visible,
                    enabled: c.enabled,
                    href: c.href.clone(),
                    expanded: c.expanded,
                    hints: c.hints.clone(),
                }],
            })
            .collect();
        if self.reverse {
            comps.reverse();
        }
        View {
            url: page.url.clone(),
            components: comps,
            modal: page.modal.clone(),
            frames: vec!["main".to_string()],
        }
    }
}

impl Browser for Fake {
    fn load(&mut self, url: &str) -> Result<View, StepError> {
        std::thread::sleep(self.load_delay);
        self.loaded.push(url.to_string());
        self.requests += self.per_request;
        // A page behind a dialog cannot be loaded by its address.
        let found = self
            .pages
            .iter()
            .find(|(_, page)| page.url == url && page.modal.is_none())
            .map(|(key, _)| *key);
        match found {
            Some(key) => {
                self.current = key;
                self.stranded = false;
                Ok(self.view())
            }
            None => {
                self.stranded = true;
                Err(StepError::Failed(format!("no page at {url}")))
            }
        }
    }

    fn act(&mut self, instance: &ViewInstance, act: &Act, _timeout_ms: u64) -> Result<Acted, StepError> {
        std::thread::sleep(self.delay);
        if self.stranded {
            return Err(StepError::Fatal("no session is open".into()));
        }
        let id = instance.selector.trim_start_matches("sel:").to_string();
        self.acted.push(id.clone());
        self.kinds.push(format!("{} {id}", act.name()));
        if let Act::Press(key) = act {
            self.keys.push((*key).to_string());
        }
        if let Act::Type(text) = act {
            self.typed.push(text.clone());
        }
        self.requests += self.per_request;
        if let Some((after, flag)) = self.cancel {
            if self.acted.len() >= after {
                flag.store(true, Ordering::SeqCst);
            }
        }
        let comp = self.pages[self.current]
            .comps
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| StepError::NotFound(id.clone()))?;
        if comp.click_blocked && matches!(act, Act::Click) {
            return Err(StepError::Failed("click is blocked".into()));
        }
        if let (Some(error), Act::Press(_)) = (comp.press_fails.clone(), act) {
            return Err(error);
        }
        if let Some(error) = comp.fails {
            return Err(error);
        }
        let before = self.pages[self.current].url.clone();
        if let Some(to) = comp.to {
            self.current = to;
        }
        let after = self.pages[self.current].url.clone();
        Ok(Acted {
            effect: Effect {
                navigated: before != after,
                changed: comp.to.is_some(),
                refused: self
                    .refuse
                    .filter(|(component, _, _)| *component == id)
                    .map(|(_, method, url)| Refused {
                        method: method.to_string(),
                        url: url.to_string(),
                    })
                    .into_iter()
                    .collect(),
                ..Effect::default()
            },
            view: self.view(),
        })
    }

    fn requests(&self) -> Option<BTreeMap<String, u64>> {
        self.count_requests
            .then(|| BTreeMap::from([(ORIGIN.to_string(), self.requests)]))
    }
}

fn options() -> Options {
    Options {
        start_url: format!("{ORIGIN}/"),
        strategy: Strategy::Bfs,
        seed: 0,
        similarity: DEFAULT_SIMILARITY,
        budgets: Budgets::default(),
        rules: Rules::new(&format!("{ORIGIN}/"), false, &[]).unwrap(),
        chrome_pages: 0,
        model: None,
    }
}

/// What a walk told its listener.
#[derive(Debug)]
struct Told {
    kind: &'static str,
    message: String,
    data: serde_json::Value,
}

fn run_told(fake: &mut Fake, options: &Options) -> (Explored, Vec<Told>) {
    static NEVER: AtomicBool = AtomicBool::new(false);
    let mut told = Vec::new();
    let done = explore(fake, options, &NEVER, &mut |event| {
        told.push(Told {
            kind: event.kind(),
            message: event.message(),
            data: event.data(),
        });
    });
    check_walk(&done, &told);
    (done, told)
}

fn run(fake: &mut Fake, options: &Options) -> Explored {
    run_told(fake, options).0
}

/// What must hold for any walk, whatever the site: its parts agree with each other. Every walk of this file is checked.
fn check_walk(done: &Explored, told: &[Told]) {
    let graph = &done.graph;
    // One action for each edge that was made, and the actions of the replays that reopened a screen, which make no edge.
    let made = graph.edges.iter().filter(|e| e.status != Status::Missing).count();
    assert!(graph.actions as usize >= made, "{:?}", graph.edges);

    let of = |kind: &str| -> Vec<&Told> { told.iter().filter(|t| t.kind == kind).collect() };
    // Events: exactly one end for each edge, in order, and a start only for an edge that has an end.
    let ends: Vec<&str> = of("explore.action.end")
        .iter()
        .map(|t| t.data["edge"].as_str().unwrap())
        .collect();
    let edges: Vec<&str> = graph.edges.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ends, edges, "one explore.action.end for each edge");
    // A start has its end, except for the last action of a walk that did not finish: the browser was lost in it, or
    // it would have found one screen too many.
    let starts = of("explore.action.start");
    let dangling: Vec<usize> = starts
        .iter()
        .enumerate()
        .filter(|(_, start)| !edges.contains(&start.data["edge"].as_str().unwrap()))
        .map(|(n, _)| n)
        .collect();
    assert!(dangling.len() <= 1, "{dangling:?}");
    if let Some(&n) = dangling.first() {
        assert_eq!(n, starts.len() - 1, "only the last start can be without an edge");
        assert_ne!(graph.stopped, Stop::Complete);
    }
    for edge in graph.edges.iter().filter(|e| e.status == Status::Missing) {
        assert!(
            !starts.iter().any(|t| t.data["edge"] == edge.id.as_str()),
            "{} had no action to start",
            edge.id
        );
    }
    // The actions that made no edge are the actions of replays, and every attempt of a replay is told.
    let replayed: u64 = of("explore.replay")
        .iter()
        .map(|t| t.data["actions"].as_u64().unwrap())
        .sum();
    assert_eq!(
        replayed,
        u64::from(graph.actions) - made as u64,
        "the replays account for the actions that are not edges"
    );
    // A screen is announced once, in the order it was found.
    let new: Vec<&str> = of("explore.state.new")
        .iter()
        .map(|t| t.data["state"].as_str().unwrap())
        .collect();
    let states: Vec<&str> = graph.states.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(new, states);
    // What the rules left alone is told once each.
    let blocks = of("explore.policy.block")
        .iter()
        .filter(|t| t.data["kind"] == "component")
        .count();
    assert_eq!(blocks, graph.skipped.len());

    // Coverage: totals are the sums, and every discovered component is exactly one of touched, skipped, untouched.
    let coverage = &done.coverage;
    assert_eq!(coverage.states.len(), graph.states.len());
    let totals = coverage.totals();
    let mut sums = (0, 0, 0, 0);
    for state in &coverage.states {
        let sets = |list: &[String]| list.iter().cloned().collect::<std::collections::BTreeSet<_>>();
        let (discovered, touched, untouched) = (sets(&state.discovered), sets(&state.touched), sets(&state.untouched));
        let skipped: std::collections::BTreeSet<String> = state.skipped.iter().map(|(c, _)| c.clone()).collect();
        assert_eq!(
            discovered.len(),
            state.discovered.len(),
            "a component is discovered once on a screen"
        );
        let mut union = touched.clone();
        union.extend(skipped.iter().cloned());
        union.extend(untouched.iter().cloned());
        assert_eq!(union, discovered, "{}: the parts make up the whole", state.state);
        assert_eq!(
            touched.len() + skipped.len() + untouched.len(),
            discovered.len(),
            "{}: and do not overlap",
            state.state
        );
        // Everything that was acted on is in the figures.
        let acted: std::collections::BTreeSet<&str> = graph
            .edges
            .iter()
            .filter(|e| e.from == state.state && e.status != Status::Missing)
            .map(|e| e.component.as_str())
            .collect();
        assert_eq!(
            touched
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            acted,
            "{}: touched is what was acted on",
            state.state
        );
        sums.0 += discovered.len();
        sums.1 += touched.len();
        sums.2 += skipped.len();
        sums.3 += untouched.len();
    }
    assert_eq!(
        (totals.discovered, totals.touched, totals.skipped, totals.untouched),
        sums,
        "the totals are the sums of the screens"
    );
    assert_eq!(coverage.unreachable, graph.unreachable);
    assert_eq!(coverage.stopped, graph.stopped.as_str());
    assert!(totals.touched <= totals.discovered && totals.per_mille() <= 1000);
    assert_eq!(coverage.to_bytes(), coverage.clone().to_bytes());
}

/// home -> a, b; a -> a1; b -> b1. Leaves have nothing to act on.
fn tree() -> Vec<(&'static str, P)> {
    vec![
        (
            "home",
            page(
                "/",
                vec![
                    c("Home title", "heading"),
                    C::link("A", "a", "/a"),
                    C::link("B", "b", "/b"),
                ],
            ),
        ),
        (
            "a",
            page("/a", vec![c("A title", "heading"), C::link("A1", "a1", "/a1")]),
        ),
        (
            "b",
            page("/b", vec![c("B title", "heading"), C::link("B1", "b1", "/b1")]),
        ),
        ("a1", leaf("/a1", "A1 title")),
        ("b1", leaf("/b1", "B1 title")),
    ]
}

/// A chain home -> p1 -> p2 -> ... -> p`n`, each page with its own link and title.
fn chain(n: usize) -> Vec<(&'static str, P)> {
    let key = |i: usize| -> &'static str { Box::leak(format!("p{i}").into_boxed_str()) };
    let mut pages = Vec::new();
    for i in 0..=n {
        let here = if i == 0 { "home" } else { key(i) };
        let path = if i == 0 { "/".to_string() } else { format!("/p{i}") };
        let mut comps = vec![c(&format!("Title {i}"), "heading")];
        if i < n {
            comps.push(C::link(&format!("Next {i}"), key(i + 1), &format!("/p{}", i + 1)));
        }
        pages.push((here, page(&path, comps)));
    }
    pages
}

// ---------------------------------------------------------------------------------------------------------------
// Order and determinism
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn breadth_first_takes_everything_one_step_away_before_going_deeper() {
    let mut fake = Fake::new(tree());
    let done = run(&mut fake, &options());
    assert_eq!(fake.acted, ["c_a", "c_b", "c_a1", "c_b1"]);
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(done.graph.states.len(), 5);
}

#[test]
fn depth_first_goes_down_before_across() {
    let mut fake = Fake::new(tree());
    let mut opts = options();
    opts.strategy = Strategy::Dfs;
    run(&mut fake, &opts);
    assert_eq!(fake.acted, ["c_a", "c_a1", "c_b", "c_b1"]);
}

#[test]
fn candidates_are_ordered_by_path_then_id_whatever_order_the_page_lists_them_in() {
    let make = || {
        vec![(
            "home",
            page(
                "/",
                vec![
                    C::button("Zed", "home").at("main > b"),
                    C::link("Mid", "home", "/").at("main > a"),
                    C::link("Alpha", "home", "/").at("main > a"),
                    C::link("Top", "home", "/").at("header"),
                ],
            ),
        )]
    };
    let mut forward = Fake::new(make());
    let mut backward = Fake::new(make());
    backward.reverse = true;
    let first = run(&mut forward, &options());
    let second = run(&mut backward, &options());
    // header < main > a < main > b, and inside main > a the ids c_alpha < c_mid.
    assert_eq!(forward.acted, ["c_top", "c_alpha", "c_mid"]);
    assert_eq!(forward.acted, backward.acted);
    assert_eq!(first.graph.to_bytes(), second.graph.to_bytes());
}

#[test]
fn the_same_walk_writes_the_same_bytes() {
    let one = run(&mut Fake::new(tree()), &options()).graph.to_bytes();
    let two = run(&mut Fake::new(tree()), &options()).graph.to_bytes();
    assert_eq!(one, two);
}

#[test]
fn states_and_edges_are_named_in_the_order_they_were_found() {
    let done = run(&mut Fake::new(tree()), &options());
    let states: Vec<&str> = done.graph.states.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(states, ["s0", "s1", "s2", "s3", "s4"]);
    let edges: Vec<(&str, &str, Option<&str>)> = done
        .graph
        .edges
        .iter()
        .map(|e| (e.id.as_str(), e.from.as_str(), e.to.as_deref()))
        .collect();
    assert_eq!(
        edges,
        [
            ("e0", "s0", Some("s1")),
            ("e1", "s0", Some("s2")),
            ("e2", "s1", Some("s3")),
            ("e3", "s2", Some("s4"))
        ]
    );
    assert_eq!(done.graph.states[3].depth, 2);
}

#[test]
fn two_links_to_the_same_page_make_one_screen_and_two_edges() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page("/", vec![C::link("One", "x", "/x"), C::link("Two", "x", "/x")]),
        ),
        ("x", leaf("/x", "X title")),
    ]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.states.len(), 2);
    assert_eq!(done.graph.edges.len(), 2);
}

#[test]
fn hidden_and_disabled_controls_are_not_tried() {
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![
                C::link("Seen", "home", "/"),
                C::link("Hidden", "home", "/").hidden(),
                C::link("Off", "home", "/").disabled(),
            ],
        ),
    )]);
    run(&mut fake, &options());
    assert_eq!(fake.acted, ["c_seen"]);
}

// ---------------------------------------------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn max_actions_stops_the_walk_at_the_limit() {
    let mut fake = Fake::new(tree());
    let mut opts = options();
    opts.budgets.max_actions = 3;
    let done = run(&mut fake, &opts);
    assert_eq!(fake.acted.len(), 3);
    assert_eq!(done.graph.actions, 3);
    assert_eq!(done.graph.stopped, Stop::MaxActions);
    assert_eq!(done.graph.edges.len(), 3);
}

#[test]
fn a_walk_that_needs_exactly_the_limit_is_complete_not_stopped() {
    let mut fake = Fake::new(tree());
    let mut opts = options();
    opts.budgets.max_actions = 4;
    assert_eq!(run(&mut fake, &opts).graph.stopped, Stop::Complete);
}

#[test]
fn max_states_stops_before_the_screen_over_the_limit_is_kept() {
    let mut fake = Fake::new(chain(10));
    let mut opts = options();
    opts.budgets.max_states = 4;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.states.len(), 4);
    assert_eq!(done.graph.stopped, Stop::MaxStates);
    // The action that found the fifth screen is not an edge to a screen that is not in the file.
    assert!(done.graph.edges.iter().all(|e| e.to.as_deref().is_none_or(|to| done
        .graph
        .states
        .iter()
        .any(|s| s.id == to))));
}

#[test]
fn max_depth_keeps_screens_no_deeper_than_the_limit_and_says_so() {
    let mut fake = Fake::new(chain(10));
    let mut opts = options();
    opts.budgets.max_depth = 3;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::MaxDepth);
    assert_eq!(done.graph.states.iter().map(|s| s.depth).max(), Some(3));
    assert_eq!(done.graph.states.len(), 4);
}

#[test]
fn a_site_shallower_than_max_depth_is_complete() {
    let mut opts = options();
    opts.budgets.max_depth = 2;
    assert_eq!(run(&mut Fake::new(tree()), &opts).graph.stopped, Stop::Complete);
}

#[test]
fn the_wall_clock_budget_stops_the_walk() {
    let mut fake = Fake::new(chain(50));
    fake.delay = Duration::from_millis(30);
    let mut opts = options();
    opts.budgets.timeout = Duration::from_millis(100);
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::Timeout);
    assert!(fake.acted.len() < 10, "{}", fake.acted.len());
    assert!(!fake.acted.is_empty());
}

#[test]
fn the_request_budget_stops_the_walk_when_one_origin_has_had_too_many() {
    let mut fake = Fake::new(chain(30));
    fake.per_request = 10;
    let mut opts = options();
    opts.budgets.max_requests_per_origin = 100;
    opts.budgets.max_depth = 100;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::MaxRequests);
    // Checked after every step, so it can go over by what one step costs and no more.
    assert!(fake.requests <= 100 + 3 * fake.per_request, "{}", fake.requests);
}

#[test]
fn a_browser_that_cannot_count_requests_does_not_stop_the_walk() {
    let mut fake = Fake::new(tree());
    fake.count_requests = false;
    let mut opts = options();
    opts.budgets.max_requests_per_origin = 1;
    assert_eq!(run(&mut fake, &opts).graph.stopped, Stop::Complete);
}

#[test]
fn actions_that_keep_timing_out_end_the_walk_but_one_that_works_resets_the_count() {
    let slow = |n: usize| -> Vec<C> {
        (0..n)
            .map(|i| C::link(&format!("Slow {i:02}"), "home", "/").fails(StepError::Timeout("slow".into())))
            .collect()
    };
    // Four timeouts, one that works, four more: never five in a row.
    let mut comps: Vec<C> = slow(4).into_iter().map(|c| c.at("a")).collect();
    comps.push(C::link("Fine", "home", "/").at("b"));
    comps.extend((4..8).map(|i| {
        C::link(&format!("Slow {i:02}"), "home", "/")
            .fails(StepError::Timeout("slow".into()))
            .at("c")
    }));
    let mut fake = Fake::new(vec![("home", page("/", comps))]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(
        done.graph.edges.iter().filter(|e| e.status == Status::Timeout).count(),
        8
    );

    let mut fake = Fake::new(vec![("home", page("/", slow(8)))]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.stopped, Stop::ActionTimeout);
    // Each timed-out click is tried once more with a key, so count the clicks.
    let clicks = fake.kinds.iter().filter(|k| k.starts_with("click")).count();
    assert_eq!(clicks as u32, MAX_TIMEOUTS_IN_A_ROW);
}

// ---------------------------------------------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn cancel_stops_after_the_action_that_was_running_and_keeps_what_was_found() {
    static FLAG: AtomicBool = AtomicBool::new(false);
    let mut fake = Fake::new(tree());
    fake.cancel = Some((2, &FLAG));
    let done = explore(&mut fake, &options(), &FLAG, &mut |_| {});
    assert_eq!(done.graph.stopped, Stop::Cancelled);
    assert_eq!(
        fake.acted.len(),
        2,
        "the second action finished and nothing after it started"
    );
    assert_eq!(done.graph.edges.len(), 2);
    assert!(done.graph.states.len() >= 3);
}

#[test]
fn cancel_before_the_start_opens_nothing() {
    static FLAG: AtomicBool = AtomicBool::new(true);
    let mut fake = Fake::new(tree());
    let done = explore(&mut fake, &options(), &FLAG, &mut |_| {});
    assert_eq!(done.graph.stopped, Stop::Cancelled);
    assert!(fake.loaded.is_empty());
}

// ---------------------------------------------------------------------------------------------------------------
// Safety
// ---------------------------------------------------------------------------------------------------------------

fn risky() -> Vec<(&'static str, P)> {
    vec![
        (
            "home",
            page(
                "/",
                vec![
                    C::link("Products", "home", "/"),
                    C::link("Delete account", "home", "/"),
                    C::link("Pay now", "home", "/"),
                    C::link("Elsewhere", "home", "https://other.test/"),
                    C::link("Log out", "home", "/logout"),
                    C::link("Mail us", "home", "mailto:a@b.test"),
                    C::button("Continue", "home"),
                    c("Search", "searchbox"),
                    C::button("Menu", "home").expanded(),
                    C::button("Clear cart", "home"),
                ],
            ),
        ),
        ("x", leaf("/x", "X")),
    ]
}

#[test]
fn nothing_the_rules_skip_is_ever_acted_on() {
    for writes in [false, true] {
        let mut fake = Fake::new(risky());
        let mut opts = options();
        opts.rules = Rules::new(&opts.start_url, writes, &[]).unwrap();
        run(&mut fake, &opts);
        for forbidden in [
            "c_delete_account",
            "c_pay_now",
            "c_elsewhere",
            "c_log_out",
            "c_mail_us",
            "c_clear_cart",
        ] {
            assert!(
                !fake.acted.contains(&forbidden.to_string()),
                "writes={writes}: {forbidden} was acted on: {:?}",
                fake.acted
            );
        }
    }
}

#[test]
fn read_only_touches_links_and_things_that_open_and_close_and_nothing_else() {
    let mut fake = Fake::new(risky());
    run(&mut fake, &options());
    assert_eq!(fake.acted, ["c_menu", "c_products"]);
}

#[test]
fn with_writes_allowed_a_misleading_button_is_tried_but_a_destructive_name_still_is_not() {
    let mut fake = Fake::new(risky());
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    run(&mut fake, &opts);
    assert!(fake.acted.contains(&"c_continue".to_string()), "{:?}", fake.acted);
    assert!(fake.acted.contains(&"c_search".to_string()));
}

#[test]
fn what_was_skipped_is_written_down_with_the_reason() {
    let done = run(&mut Fake::new(risky()), &options());
    let reason = |id: &str| {
        done.graph
            .skipped
            .iter()
            .find(|s| s.component == id)
            .map(|s| s.reason.as_str())
    };
    assert_eq!(reason("c_delete_account"), Some("destructive"));
    assert_eq!(reason("c_clear_cart"), Some("destructive"));
    assert_eq!(reason("c_elsewhere"), Some("blocked"));
    assert_eq!(reason("c_continue"), Some("read-only"));
    assert_eq!(reason("c_search"), Some("read-only"));
    assert_eq!(reason("c_products"), None);
    // Once, however many times the screen is opened.
    assert_eq!(
        done.graph
            .skipped
            .iter()
            .filter(|s| s.component == "c_delete_account")
            .count(),
        1
    );
}

#[test]
fn a_denied_name_added_by_the_person_is_skipped_too() {
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![C::link("Contact us", "home", "/"), C::link("Products", "home", "/")],
        ),
    )]);
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, false, &["contact".to_string()]).unwrap();
    run(&mut fake, &opts);
    assert_eq!(fake.acted, ["c_products"]);
}

#[test]
fn text_is_typed_only_with_writes_allowed_and_the_value_is_the_same_each_run() {
    let site = || {
        vec![(
            "home",
            page("/", vec![c("Your email", "textbox"), C::link("Go", "home", "/")]),
        )]
    };
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    let mut fake = Fake::new(site());
    run(&mut fake, &opts);
    assert!(
        fake.kinds.contains(&"type c_your_email".to_string()),
        "{:?}",
        fake.kinds
    );

    let mut read_only = Fake::new(site());
    run(&mut read_only, &options());
    assert!(read_only.kinds.iter().all(|k| !k.starts_with("type")));
}

#[test]
fn a_select_is_left_alone_because_the_walker_does_not_know_its_options() {
    let mut fake = Fake::new(vec![(
        "home",
        page("/", vec![c("Size", "combobox"), C::link("Go", "home", "/")]),
    )]);
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    let done = run(&mut fake, &opts);
    assert_eq!(fake.acted, ["c_go"]);
    assert!(done
        .graph
        .skipped
        .iter()
        .any(|s| s.component == "c_size" && s.reason == "no-options"));
}

// ---------------------------------------------------------------------------------------------------------------
// Reading, failing and getting back
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn a_page_with_nothing_on_it_is_a_failed_read_not_a_screen() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![C::link("Blank", "blank", "/blank"), C::link("Fine", "fine", "/fine")],
            ),
        ),
        ("blank", page("/blank", vec![c("Hidden", "heading").hidden()])),
        ("fine", leaf("/fine", "Fine title")),
    ]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.states.len(), 2, "home and fine only");
    let blank = done.graph.edges.iter().find(|e| e.component == "c_blank").unwrap();
    assert_eq!(blank.status, Status::Failed);
    assert_eq!(blank.to, None);
}

#[test]
fn a_start_page_with_nothing_on_it_ends_the_walk_with_an_error() {
    let mut fake = Fake::new(vec![("home", page("/", vec![c("Hidden", "heading").hidden()]))]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.stopped, Stop::Error);
    assert!(done.error.is_some());
    assert!(done.graph.states.is_empty());
}

#[test]
fn a_start_page_that_will_not_load_ends_the_walk_with_an_error() {
    let mut fake = Fake::new(vec![("other", leaf("/other", "Other"))]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.stopped, Stop::Error);
    assert!(done.error.unwrap().contains("no page"));
}

#[test]
fn a_browser_failure_in_the_middle_keeps_what_was_found_and_says_error() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![
                    C::link("A", "a", "/a"),
                    C::link("B", "home", "/").fails(StepError::Fatal("gone".into())),
                ],
            ),
        ),
        ("a", leaf("/a", "A title")),
    ]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.stopped, Stop::Error);
    assert_eq!(done.error.as_deref(), Some("gone"));
    assert_eq!(done.graph.states.len(), 2);
}

/// home has "Open", which shows a dialog at the same address; the dialog has two links back to home. The dialog has no
/// address of its own.
fn shop_with_a_dialog() -> Vec<(&'static str, P)> {
    let mut dialog = page(
        "/",
        vec![
            c("Dialog title", "heading"),
            C::link("Yes", "home", "/"),
            C::link("Later", "home", "/"),
        ],
    );
    dialog.modal = Some("c_dialog".to_string());
    vec![
        (
            "home",
            page(
                "/",
                vec![C::link("Open", "dialog", "/open"), C::link("About", "about", "/about")],
            ),
        ),
        ("dialog", dialog),
        ("about", leaf("/about", "About title")),
    ]
}

#[test]
fn a_screen_with_no_address_is_reopened_by_replaying_the_actions_that_led_to_it() {
    // The walk finds the dialog by clicking "Open" and tries "Later" there. For "Yes" it must come back: the address
    // shows the home page, so it opens the home page and clicks "Open" again.
    let mut fake = Fake::new(shop_with_a_dialog());
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(done.graph.states.len(), 3);
    assert_eq!(done.graph.unreachable, Vec::<String>::new());
    assert_eq!(fake.acted, ["c_about", "c_open", "c_later", "c_open", "c_yes"]);
    // Four edges and one replayed action.
    assert_eq!(done.graph.edges.len(), 4);
    assert_eq!(done.graph.actions, 5);
}

#[test]
fn a_screen_the_address_cannot_bring_back_is_not_tried_by_address_again() {
    // The address is tried twice, once, for the dialog. Every later reset goes straight to the replay.
    let mut fake = Fake::new(shop_with_a_dialog());
    let mut opts = options();
    // Two more links in the dialog make three resets to it.
    fake.pages
        .get_mut("dialog")
        .unwrap()
        .comps
        .push(C::link("Maybe", "home", "/"));
    opts.budgets.max_actions = 50;
    run(&mut fake, &opts);
    // Loads: the start, the dialog's address twice, then one start page for each replay (the third and fourth control).
    let at_start = fake
        .loaded
        .iter()
        .filter(|u| u.as_str() == format!("{ORIGIN}/"))
        .count();
    assert_eq!(
        fake.loaded.len(),
        at_start,
        "only the start page is ever opened: {:?}",
        fake.loaded
    );
    assert_eq!(
        fake.acted.iter().filter(|a| *a == "c_open").count(),
        3,
        "{:?}",
        fake.acted
    );
}

#[test]
fn replayed_actions_count_against_max_actions() {
    // "About", "Open", "Later" and then one replayed "Open" is four actions: the fifth, "Yes", is not allowed.
    let mut fake = Fake::new(shop_with_a_dialog());
    let mut opts = options();
    opts.budgets.max_actions = 4;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::MaxActions);
    assert_eq!(done.graph.actions, 4);
    assert_eq!(fake.acted, ["c_about", "c_open", "c_later", "c_open"]);

    // Stopping while replaying is a stop and not a failure to reopen.
    let mut fake = Fake::new(shop_with_a_dialog());
    opts.budgets.max_actions = 3;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::MaxActions);
    assert_eq!(done.graph.unreachable, Vec::<String>::new());
}

#[test]
fn a_replay_follows_every_step_of_a_chain_in_order() {
    // home -> menu (same address) -> drawer (same address). The drawer is reopened by the two actions that led to it.
    let drawer = |title: &str| {
        let mut p = page(
            "/",
            vec![
                c(title, "heading"),
                C::link("Inner one", "home", "/"),
                C::link("Inner two", "home", "/"),
            ],
        );
        p.modal = Some("c_dialog".to_string());
        p
    };
    let mut menu = page("/", vec![c("Menu title", "heading"), C::link("Deeper", "drawer", "/")]);
    menu.modal = Some("c_menu".to_string());
    let mut fake = Fake::new(vec![
        ("home", page("/", vec![C::link("Menu", "menu", "/")])),
        ("menu", menu),
        ("drawer", drawer("Drawer title")),
    ]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.unreachable, Vec::<String>::new());
    // Replay of the drawer: the menu, then deeper.
    assert_eq!(
        fake.acted,
        ["c_menu", "c_deeper", "c_inner_one", "c_menu", "c_deeper", "c_inner_two"],
        "{:?}",
        fake.acted
    );
}

#[test]
fn a_screen_that_cannot_be_reopened_by_replay_either_is_listed_and_the_walk_goes_on() {
    // The dialog only comes the first time: the second "Open" does nothing.
    struct Once(Fake, u32);
    impl Browser for Once {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.0.load(url)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            if i.selector == "sel:c_open" {
                self.1 += 1;
                if self.1 > 1 {
                    return Ok(Acted {
                        effect: Effect::default(),
                        view: self.0.view(),
                    });
                }
            }
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut browser = Once(Fake::new(shop_with_a_dialog()), 0);
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut browser, &options(), &NEVER, &mut |_| {});
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(done.graph.unreachable, ["s2"]);
    // Tried: About, Open, Later. Then two replays of "Open" (two attempts, no more) and "Yes" never.
    assert_eq!(
        browser.0.acted,
        ["c_about", "c_open", "c_later"],
        "{:?}",
        browser.0.acted
    );
    assert_eq!(browser.1, 3, "the dialog's link was clicked once and replayed twice");
}

#[test]
fn a_replay_that_goes_wrong_halfway_stops_there() {
    // "Next" leads to a page that is not the one that was recorded the second time: nothing more is clicked on it.
    struct Moved(Fake, u32);
    impl Browser for Moved {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.0.load(url)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            if i.selector == "sel:c_deeper" {
                self.1 += 1;
                if self.1 > 1 {
                    self.0.current = "about";
                    return Ok(Acted {
                        effect: Effect {
                            navigated: true,
                            changed: true,
                            ..Effect::default()
                        },
                        view: self.0.view(),
                    });
                }
            }
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut drawer = page(
        "/",
        vec![
            c("Drawer title", "heading"),
            C::link("Inner one", "home", "/"),
            C::link("Inner two", "home", "/"),
        ],
    );
    drawer.modal = Some("c_dialog".to_string());
    let mut menu = page("/", vec![c("Menu title", "heading"), C::link("Deeper", "drawer", "/")]);
    menu.modal = Some("c_menu".to_string());
    let mut browser = Moved(
        Fake::new(vec![
            ("home", page("/", vec![C::link("Menu", "menu", "/")])),
            ("menu", menu),
            ("drawer", drawer),
            ("about", leaf("/about", "About title")),
        ]),
        0,
    );
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut browser, &options(), &NEVER, &mut |_| {});
    assert_eq!(done.graph.unreachable, ["s2"]);
    assert!(
        !browser.0.acted.iter().skip(4).any(|a| a.starts_with("c_inner")),
        "nothing was clicked on a page that was not the drawer: {:?}",
        browser.0.acted
    );
}

#[test]
fn a_chain_with_typed_text_is_never_replayed() {
    // The dialog opens only after text is typed. Typing again could do its harm again, so the dialog is given up.
    let mut dialog = page(
        "/",
        vec![
            c("Dialog title", "heading"),
            C::link("Yes", "home", "/"),
            C::link("Later", "home", "/"),
        ],
    );
    dialog.modal = Some("c_dialog".to_string());
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![C {
                    to: Some("dialog"),
                    ..c("Search", "textbox")
                }],
            ),
        ),
        ("dialog", dialog),
    ]);
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.unreachable, ["s1"]);
    assert_eq!(fake.kinds, ["type c_search", "click c_later"], "{:?}", fake.kinds);
}

#[test]
fn the_rules_are_asked_again_for_every_replayed_action() {
    // The page that opens the dialog starts to show a button that the rules skip, in place of the link that was
    // clicked before. The replay must not click it.
    struct Swapped(Fake, u32);
    impl Browser for Swapped {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.1 += 1;
            if self.0.acted.iter().any(|a| a == "c_open") {
                let comps = &mut self.0.pages.get_mut("home").unwrap().comps;
                comps.retain(|c| c.id != "c_open");
                comps.push(C {
                    id: "c_open".to_string(),
                    ..C::button("Delete everything", "dialog")
                });
            }
            self.0.load(url)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut browser = Swapped(Fake::new(shop_with_a_dialog()), 0);
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut browser, &options(), &NEVER, &mut |_| {});
    assert_eq!(done.graph.unreachable, ["s2"]);
    assert_eq!(
        browser.0.acted.iter().filter(|a| *a == "c_open").count(),
        1,
        "{:?}",
        browser.0.acted
    );
}

#[test]
fn a_walk_with_a_replay_writes_the_same_bytes_each_time() {
    let one = run(&mut Fake::new(shop_with_a_dialog()), &options()).graph.to_bytes();
    let two = run(&mut Fake::new(shop_with_a_dialog()), &options()).graph.to_bytes();
    assert_eq!(one, two);
}

#[test]
fn the_page_is_not_opened_again_when_the_walk_is_already_standing_where_it_needs_to_be() {
    // Each new screen is explored where the walk found it.
    let mut fake = Fake::new(chain(4));
    run(&mut fake, &options());
    assert_eq!(fake.loaded.len(), 1, "{:?}", fake.loaded);

    // A control that does nothing leaves the page as it was.
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![c("Dead", "button").at("a"), C::link("Live", "home", "/").at("b")],
        ),
    )]);
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    run(&mut fake, &opts);
    assert_eq!(fake.acted, ["c_dead", "c_live"]);
    assert_eq!(fake.loaded.len(), 1, "{:?}", fake.loaded);
}

#[test]
fn a_page_opened_again_is_the_screen_when_it_is_alike_even_if_another_screen_is_closer() {
    // Two screens at one address that are not alike: the page, and the page with four sections more showing. The page
    // opened again shows two of them: within 90% of the first, a little closer to the second. It is still the first.
    struct Between(Fake, u32);
    impl Browser for Between {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.1 += 1;
            let view = self.0.load(url)?;
            if self.1 == 1 {
                return Ok(view);
            }
            self.0.current = "zbetween";
            Ok(self.0.view())
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let base = |extra: usize| -> Vec<C> {
        let mut comps: Vec<C> = (0..20).map(|i| c(&format!("Part {i:02}"), "heading").at("a")).collect();
        comps.extend((0..extra).map(|i| c(&format!("More {i}"), "heading").at("a")));
        comps.push(C::link("Show more", "more", "/").at("b"));
        comps.push(C::link("Stay", "home", "/").at("c"));
        comps
    };
    // The first screen has 22 ids (20 and the two links). Showing more adds four (26 ids, 84.6% alike to the first).
    // The page opened again has two of them: 24 ids, 91.7% alike to the first and 92.3% to the second.
    let more = base(4);
    let mut browser = Between(
        Fake::new(vec![
            ("home", page("/", base(0))),
            ("more", page("/", more)),
            ("zbetween", page("/", base(2))),
        ]),
        0,
    );
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut browser, &options(), &NEVER, &mut |_| {});
    assert_eq!(done.graph.states.len(), 2, "{:?}", done.graph.states);
    assert_eq!(done.graph.unreachable, Vec::<String>::new());
    assert!(browser.0.acted.contains(&"c_stay".to_string()), "{:?}", browser.0.acted);
}

#[test]
fn a_screen_that_looks_different_once_when_opened_again_is_given_a_second_chance() {
    // Pages differ a little from one load to the next (a rotating item, a script that came late). One odd load must
    // not make the walk give up everything that is left on a screen.
    struct Odd(Fake, u32);
    impl Browser for Odd {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.1 += 1;
            let mut view = self.0.load(url)?;
            if self.1 == 2 {
                // The second load of the run is the odd one: a quarter of the controls did not show.
                view.components.truncate(view.components.len() * 3 / 4);
            }
            Ok(view)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut comps: Vec<C> = (0..20)
        .map(|i| c(&format!("Filler {i:02}"), "heading").at("a"))
        .collect();
    comps.extend([
        C::link("One", "home", "/").at("b"),
        C::link("Two", "home", "/").at("c"),
        C::link("Three", "home", "/").at("d"),
    ]);
    let mut browser = Odd(Fake::new(vec![("home", page("/", comps))]), 0);
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut browser, &options(), &NEVER, &mut |_| {});
    assert_eq!(done.graph.unreachable, Vec::<String>::new());
    assert_eq!(
        browser.0.acted,
        ["c_one", "c_two", "c_three"],
        "every control was tried"
    );
}

#[test]
fn a_control_that_is_gone_when_the_screen_is_opened_again_is_recorded_as_missing() {
    // The page shows a different set of controls the second time (a banner that does not come back).
    struct Changing(Fake, u32);
    impl Browser for Changing {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.1 += 1;
            if self.1 > 1 {
                self.0.pages.get_mut("home").unwrap().comps.retain(|c| c.id != "c_shop");
            }
            self.0.load(url)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut comps: Vec<C> = (0..17).map(|i| c(&format!("Filler {i:02}"), "heading")).collect();
    comps.extend([
        C::link("Banner", "home", "/").at("a"),
        C::link("Menu", "home", "/").at("b"),
        C::link("Shop", "home", "/").at("c"),
    ]);
    let mut browser = Changing(Fake::new(vec![("home", page("/", comps))]), 0);
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut browser, &options(), &NEVER, &mut |_| {});
    assert_eq!(
        browser.0.acted,
        ["c_banner", "c_menu"],
        "nothing is acted on that is not there"
    );
    let shop = done.graph.edges.iter().find(|e| e.component == "c_shop").unwrap();
    assert_eq!(shop.status, Status::Missing);
    assert_eq!(shop.to, None);
    assert_eq!(done.graph.actions, 2, "an action that was never made is not counted");
    assert_eq!(done.graph.stopped, Stop::Complete);
}

#[test]
fn a_blocked_click_is_tried_once_more_with_the_keyboard() {
    let mut blocked = C::link("Covered", "x", "/x");
    blocked.click_blocked = true;
    let mut fake = Fake::new(vec![("home", page("/", vec![blocked])), ("x", leaf("/x", "X"))]);
    let done = run(&mut fake, &options());
    assert_eq!(fake.kinds, ["click c_covered", "press c_covered"]);
    let edge = &done.graph.edges[0];
    assert_eq!(edge.action, "press");
    assert_eq!(edge.status, Status::Ok);
    assert_eq!(done.graph.actions, 1, "a click and its fallback are one action");
}

#[test]
fn when_the_keyboard_retry_fails_too_the_failure_of_the_click_is_the_one_reported() {
    // A click that runs out of time on a link to a page that never answers leaves the page navigating, so the key press
    // finds nothing. "Not found" would hide the timeout.
    let mut slow = C::link("Slow page", "home", "/slow");
    // Only the click times out; the press finds the page gone.
    slow.fails = Some(StepError::Timeout("click did not finish".into()));
    slow.press_fails = Some(StepError::NotFound("gone".into()));
    let mut fake = Fake::new(vec![("home", page("/", vec![slow]))]);
    let done = run(&mut fake, &options());
    let edge = &done.graph.edges[0];
    assert_eq!(edge.status, Status::Timeout, "{edge:?}");
    assert_eq!(edge.action, "click");
    assert_eq!(
        done.graph.actions, 1,
        "it was an action: it took time and the site was asked"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// What the first pages tell about the header and footer
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_extra_pages_read_first_are_links_the_rules_allow_and_are_counted() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![
                    C::link("One", "one", "/one"),
                    C::link("Two", "two", "/two"),
                    C::link("Three", "three", "/three"),
                    C::link("Out", "home", "/logout"),
                    C::link("Away", "home", "https://other.test/x"),
                ],
            ),
        ),
        ("one", leaf("/one", "One")),
        ("two", leaf("/two", "Two")),
        ("three", leaf("/three", "Three")),
    ]);
    let mut opts = options();
    opts.chrome_pages = 2;
    // No action is made, so every load is one of the page reads.
    opts.budgets.max_actions = 0;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.chrome_pages, 3, "the start page and two more");
    assert_eq!(
        fake.loaded[..3],
        [format!("{ORIGIN}/"), format!("{ORIGIN}/one"), format!("{ORIGIN}/three")],
        "ordered by path and id: c_one, c_three, c_two. The start page first"
    );
    assert_eq!(
        fake.loaded.len(),
        4,
        "the browser is somewhere else now, so the start page is opened again: {:?}",
        fake.loaded
    );
    assert_eq!(fake.loaded[3], format!("{ORIGIN}/"));
    assert!(fake
        .loaded
        .iter()
        .all(|u| !u.contains("logout") && !u.contains("other.test")));
}

#[test]
fn a_header_link_that_is_on_every_page_is_tried_once_and_not_on_every_screen() {
    let header = |at: &str| -> Vec<C> {
        vec![
            C::link("Nav home", "home", "/").at("header"),
            C::link("Nav about", "about", "/about").at("header"),
            C::link("Nav shop", "shop", "/shop").at("header"),
        ]
        .into_iter()
        .map(|c| c.at(at))
        .collect()
    };
    let mut home = header("header");
    home.extend([
        c("Home title", "heading"),
        C::link("Deals", "deals", "/deals").at("main"),
    ]);
    let mut about = header("header");
    about.extend([c("About title", "heading")]);
    let mut shop = header("header");
    shop.extend([c("Shop title", "heading")]);
    let mut deals = header("header");
    deals.extend([c("Deals title", "heading"), c("Deals extra", "heading")]);
    let site = || {
        vec![
            ("home", page("/", home.clone())),
            ("about", page("/about", about.clone())),
            ("shop", page("/shop", shop.clone())),
            ("deals", page("/deals", deals.clone())),
        ]
    };
    let mut opts = options();
    opts.chrome_pages = 3;
    let mut fake = Fake::new(site());
    let done = run(&mut fake, &opts);
    assert_eq!(
        done.graph.chrome.len(),
        3,
        "the three header links are page-wide: {:?}",
        done.graph.chrome
    );
    for nav in ["c_nav_home", "c_nav_about", "c_nav_shop"] {
        assert_eq!(
            fake.acted.iter().filter(|id| *id == nav).count(),
            1,
            "{nav} is tried once, not on every screen: {:?}",
            fake.acted
        );
    }
    assert!(fake.acted.contains(&"c_deals".to_string()));

    // Without the page-wide ids the same links are tried on every screen, which is what costs the budget.
    let mut none = options();
    none.chrome_pages = 0;
    let mut again = Fake::new(site());
    run(&mut again, &none);
    assert!(again.acted.iter().filter(|id| *id == "c_nav_about").count() > 1);
}

#[test]
fn a_header_link_first_seen_on_a_screen_too_deep_to_explore_is_still_tried_where_it_can_be() {
    // The header is on the start page too, so it is tried there; and a site whose start page is the deep one
    // still tries it on the first screen that is shallow enough.
    let nav = || C::link("Nav shop", "shop", "/shop").at("header");
    let site = vec![
        (
            "home",
            page(
                "/",
                vec![nav(), nav().at("header 2"), C::link("Go", "deep", "/deep").at("main")],
            ),
        ),
        ("deep", page("/deep", vec![nav(), c("Deep title", "heading")])),
        ("shop", page("/shop", vec![nav(), c("Shop title", "heading")])),
    ];
    let mut opts = options();
    opts.chrome_pages = 2;
    opts.budgets.max_depth = 1;
    let mut fake = Fake::new(site);
    run(&mut fake, &opts);
    assert!(fake.acted.contains(&"c_nav_shop".to_string()), "{:?}", fake.acted);
}

#[test]
fn the_extra_pages_are_each_read_once_and_the_start_page_is_not_one_of_them() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![
                    C::link("Self", "home", "/").at("a"),
                    C::link("Again one", "p", "/a").at("b"),
                    C::link("Again two", "p", "/a").at("c"),
                    C::link("Other", "q", "/b").at("d"),
                ],
            ),
        ),
        ("p", leaf("/a", "P")),
        ("q", leaf("/b", "Q")),
    ]);
    let mut opts = options();
    opts.chrome_pages = 5;
    opts.budgets.max_actions = 0;
    run(&mut fake, &opts);
    assert_eq!(
        fake.loaded,
        [
            format!("{ORIGIN}/"),
            format!("{ORIGIN}/a"),
            format!("{ORIGIN}/b"),
            format!("{ORIGIN}/")
        ],
        "{:?}",
        fake.loaded
    );
}

#[test]
fn a_click_that_is_blocked_is_retried_with_the_key_that_does_what_a_click_does() {
    let blocked = |c: C| C {
        click_blocked: true,
        ..c
    };
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![
                blocked(C::link("Aaa", "home", "/").at("a")),
                blocked(c("Bbb", "checkbox").at("b")),
                blocked(c("Ccc", "radio").at("c")),
                blocked(c("Ddd", "switch").at("d")),
                blocked(C::button("Eee", "home").at("e")),
            ],
        ),
    )]);
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    run(&mut fake, &opts);
    assert_eq!(fake.keys, ["Enter", "Space", "Space", "Space", "Enter"]);
}

#[test]
fn the_person_watching_is_told_what_happens_and_why_something_did_not_work() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![
                    C::link("Fine", "next", "/next").at("a"),
                    C::link("Broken", "home", "/")
                        .at("b")
                        .fails(StepError::Failed("the browser said no".into())),
                ],
            ),
        ),
        ("next", leaf("/next", "Next")),
    ]);
    let mut lines: Vec<String> = Vec::new();
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut fake, &options(), &NEVER, &mut |event| lines.push(event.message()));
    assert_eq!(done.graph.actions, 2, "one that worked and one that did not");
    assert!(lines.iter().any(|l| l.starts_with("s1 is new (depth 1)")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l == "e0: click link \"Fine\" on s0 -> s1"),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "e1: click link \"Broken\" on s0 -> failed (the browser said no)"),
        "{lines:?}"
    );
}

#[test]
fn a_label_is_the_role_and_the_name_on_one_line_and_not_long() {
    let long = "x".repeat(100);
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![
                C::link("  Add   to\n cart ", "home", "/").at("a"),
                C::link(&long, "home", "/").at("b"),
            ],
        ),
    )]);
    let done = run(&mut fake, &options());
    assert_eq!(done.graph.edges[0].label, "link \"Add to cart\"");
    assert_eq!(done.graph.edges[1].label, format!("link \"{}\"", "x".repeat(60)));
}

#[test]
fn an_extra_page_that_fails_to_load_does_not_leave_the_walk_acting_in_a_session_that_is_gone() {
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![
                    C::link("Broken", "home", "/missing").at("a"),
                    C::link("Fine", "p", "/p").at("b"),
                ],
            ),
        ),
        ("p", leaf("/p", "P")),
    ]);
    let mut opts = options();
    opts.chrome_pages = 1; // the first link, which does not load
    let done = run(&mut fake, &opts);
    assert_eq!(done.error, None, "{:?}", done.graph.edges);
    assert_eq!(done.graph.stopped, Stop::Complete);
    // The browser had been somewhere else, so the start page is opened again before anything is done on it.
    assert_eq!(
        fake.loaded[..3],
        [format!("{ORIGIN}/"), format!("{ORIGIN}/missing"), format!("{ORIGIN}/")]
    );
}

#[test]
fn without_extra_pages_only_the_start_page_is_read_first() {
    let mut fake = Fake::new(tree());
    let mut opts = options();
    opts.budgets.max_actions = 1;
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.chrome_pages, 1);
}

// ---------------------------------------------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_file_has_the_settings_the_stop_reason_and_no_empty_screen() {
    let mut opts = options();
    opts.seed = 7;
    opts.similarity = Similarity::new(0.8).unwrap();
    let done = run(&mut Fake::new(tree()), &opts);
    let json = done.graph.to_json();
    assert_eq!(json["seed"], 7);
    assert_eq!(json["strategy"], "bfs");
    assert_eq!(json["stateSimilarityPerMille"], 800);
    assert_eq!(json["stopped"], "complete");
    assert_eq!(json["budgets"]["maxActions"], 300);
    assert_eq!(json["budgets"]["maxStates"], 50);
    assert_eq!(json["budgets"]["maxDepth"], 6);
    assert_eq!(json["budgets"]["actionTimeoutMs"], 10_000);
    assert_eq!(json["budgets"]["timeoutMs"], 600_000);
    assert_eq!(json["budgets"]["maxRequestsPerOrigin"], 2000);
    for state in json["states"].as_array().unwrap() {
        assert!(!state["components"].as_array().unwrap().is_empty());
    }
    // Keys are sorted, so the bytes do not depend on the order the code builds them in.
    let text = String::from_utf8(done.graph.to_bytes()).unwrap();
    assert!(text.find("\"actions\"").unwrap() < text.find("\"budgets\"").unwrap());
    assert!(text.ends_with("}\n"));
}

#[test]
fn addresses_in_the_file_have_their_secrets_hidden() {
    let mut fake = Fake::new(vec![("home", {
        let mut p = page("/", vec![C::link("Go", "home", "/")]);
        p.url = format!("{ORIGIN}/?token=SECRET123&page=2");
        p
    })]);
    let mut opts = options();
    opts.start_url = format!("{ORIGIN}/?token=SECRET123&page=2");
    let done = run(&mut fake, &opts);
    let text = String::from_utf8(done.graph.to_bytes()).unwrap();
    assert!(!text.contains("SECRET123"), "{text}");
    assert!(text.contains("page=2"));
}

// ---------------------------------------------------------------------------------------------------------------
// A model for the text of fields (EXP-6)
// ---------------------------------------------------------------------------------------------------------------

/// A source that answers as it is told, counts its calls and keeps what it was asked.
#[derive(Debug)]
struct Said {
    reply: Result<String, String>,
    sleep: Duration,
    panics: bool,
    calls: Arc<AtomicU32>,
    asked: Arc<Mutex<Vec<Field>>>,
}

impl Said {
    fn new(reply: Result<&str, &str>) -> Self {
        Self {
            reply: reply.map(str::to_string).map_err(str::to_string),
            sleep: Duration::ZERO,
            panics: false,
            calls: Arc::new(AtomicU32::new(0)),
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn slow(mut self, sleep: Duration) -> Self {
        self.sleep = sleep;
        self
    }
    fn calls(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.calls)
    }
    fn asked(&self) -> Arc<Mutex<Vec<Field>>> {
        Arc::clone(&self.asked)
    }
}

impl ValueSource for Said {
    fn name(&self) -> &str {
        "said"
    }
    fn value(&self, field: &Field) -> Result<String, SourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.asked.lock().unwrap().push(field.clone());
        std::thread::sleep(self.sleep);
        assert!(!self.panics, "the source panicked on purpose");
        self.reply.clone().map_err(SourceError)
    }
}

/// One page with a text field for each label, in a walk that is allowed to type.
fn form(labels: &[&str]) -> Fake {
    Fake::new(vec![(
        "home",
        page(
            "/",
            labels
                .iter()
                .enumerate()
                .map(|(n, label)| c(label, "textbox").at(&format!("main > field {n:02}")))
                .collect(),
        ),
    )])
}

fn with_writes() -> Options {
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, true, &[]).unwrap();
    opts
}

fn use_model(opts: &mut Options, source: Said) {
    opts.model = Some(ModelSettings {
        source: Arc::new(source),
        max_calls: DEFAULT_MAX_MODEL_CALLS,
        timeout: DEFAULT_MODEL_TIMEOUT,
    });
}

fn outcomes(done: &Explored) -> Vec<&'static str> {
    done.model_calls.iter().map(|call| call.outcome.as_str()).collect()
}

#[test]
fn without_a_model_the_generator_types_and_no_call_is_recorded() {
    let mut fake = form(&["Search products", "Your email"]);
    let done = run(&mut fake, &with_writes());
    assert_eq!(fake.typed.len(), 2);
    assert!(done.model_calls.is_empty());
    assert_eq!(fake.typed[1], "user@example.test");
    let again = {
        let mut other = form(&["Search products", "Your email"]);
        run(&mut other, &with_writes());
        other.typed
    };
    assert_eq!(fake.typed, again, "the same seed types the same");
}

#[test]
fn the_answer_of_a_model_is_what_is_typed_and_the_call_is_recorded() {
    let source = Said::new(Ok("  blue pen "));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut fake = form(&["Search products"]);
    let done = run(&mut fake, &opts);
    assert_eq!(fake.typed, ["blue pen"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcomes(&done), ["used"]);
    let call = &done.model_calls[0];
    assert_eq!(call.answer.as_deref(), Some("blue pen"));
    assert_eq!(call.request.label, "Search products");
    assert_eq!(call.data()["source"], "said");
}

#[test]
fn a_model_is_never_asked_when_nothing_is_typed() {
    // Read-only: the field is skipped before anything is asked.
    let source = Said::new(Ok("blue pen"));
    let calls = source.calls();
    let mut opts = options();
    use_model(&mut opts, source);
    let mut fake = form(&["Search products"]);
    let done = run(&mut fake, &opts);
    assert!(fake.typed.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(done.model_calls.is_empty());
}

#[test]
fn the_cap_on_calls_ends_the_asking_and_the_generator_types_the_rest() {
    let labels = ["Field A", "Field B", "Field C", "Field D", "Field E", "Field F"];
    let source = Said::new(Ok("from the model"));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    opts.model.as_mut().unwrap().max_calls = 3;
    let mut fake = form(&labels);
    let done = run(&mut fake, &opts);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "at most the cap");
    assert_eq!(fake.typed.len(), 6);
    assert_eq!(fake.typed.iter().filter(|t| *t == "from the model").count(), 3);
    assert!(fake
        .typed
        .iter()
        .filter(|t| *t != "from the model")
        .all(|t| t.starts_with("Test input")));
    assert_eq!(outcomes(&done), ["used", "used", "used", "over-cap"]);
}

#[test]
fn a_cap_of_zero_asks_nothing() {
    let source = Said::new(Ok("from the model"));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    opts.model.as_mut().unwrap().max_calls = 0;
    let mut fake = form(&["Field A"]);
    run(&mut fake, &opts);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(fake.typed[0].starts_with("Test input"));
}

#[test]
fn a_source_that_always_fails_does_not_stop_the_walk_and_is_switched_off_after_three_calls() {
    let labels = ["Field A", "Field B", "Field C", "Field D", "Field E"];
    let source = Said::new(Err("the service is down"));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut fake = form(&labels);
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(fake.typed.len(), 5, "every field was typed into");
    assert!(fake.typed.iter().all(|t| t.starts_with("Test input")));
    assert_eq!(calls.load(Ordering::SeqCst), FAILURES_BEFORE_OFF);
    assert_eq!(outcomes(&done), ["failed", "failed", "failed"]);
    let last = done.model_calls[2].note.as_deref().unwrap();
    assert!(last.contains("switched off"), "{last}");
}

#[test]
fn a_source_that_panics_is_a_failed_call_and_not_the_end_of_the_walk() {
    let mut source = Said::new(Ok("x"));
    source.panics = true;
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut fake = form(&["Field A"]);
    let done = run(&mut fake, &opts);
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(outcomes(&done), ["failed"]);
    assert!(fake.typed[0].starts_with("Test input"));
}

#[test]
fn an_answer_that_is_empty_long_or_has_a_line_break_is_thrown_away() {
    let long = "x".repeat(300);
    for bad in ["", "   ", "line one\nline two", long.as_str(), "tab\there"] {
        let mut opts = with_writes();
        use_model(&mut opts, Said::new(Ok(bad)));
        let mut fake = form(&["Search products"]);
        let done = run(&mut fake, &opts);
        assert_eq!(outcomes(&done), ["rejected"], "{bad:?}");
        assert!(
            fake.typed[0].starts_with("Test input") || fake.typed[0].len() < 20,
            "{:?}",
            fake.typed
        );
        assert_ne!(fake.typed[0], bad);
        let kept = done.model_calls[0].answer.as_deref().unwrap_or("");
        assert!(kept.chars().count() <= 200, "the log keeps a short copy");
    }
}

#[test]
fn an_answer_that_does_not_fit_the_kind_of_field_is_thrown_away_and_one_that_does_is_typed() {
    let site = |kind: &str| {
        Fake::new(vec![(
            "home",
            page("/", vec![c("Contact", "textbox").input_type(kind)]),
        )])
    };
    for (kind, bad, good) in [
        ("email", "not an email", "ann@example.test"),
        ("number", "many", "42"),
        ("tel", "call me", "+1 (555) 0100"),
        ("url", "not a url", "https://example.test/a"),
    ] {
        let mut opts = with_writes();
        use_model(&mut opts, Said::new(Ok(bad)));
        let mut fake = site(kind);
        let done = run(&mut fake, &opts);
        assert_eq!(outcomes(&done), ["rejected"], "{kind}");
        assert_ne!(fake.typed[0], bad, "{kind}");

        let mut opts = with_writes();
        use_model(&mut opts, Said::new(Ok(good)));
        let mut fake = site(kind);
        let done = run(&mut fake, &opts);
        assert_eq!(outcomes(&done), ["used"], "{kind}");
        assert_eq!(fake.typed, [good], "{kind}");
    }
}

#[test]
fn fields_that_hold_secrets_are_never_sent() {
    let source = Said::new(Ok("from the model"));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![
                c("Your secret", "textbox").input_type("password").at("a"),
                c("Card number", "textbox").at("b"),
                c("Security code (CVV)", "textbox").at("c"),
            ],
        ),
    )]);
    let done = run(&mut fake, &opts);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing about them was sent");
    assert_eq!(fake.typed.len(), 3, "they are still typed into, with the generator");
    assert!(done.model_calls.is_empty());
}

#[test]
fn what_is_sent_is_cut_short_and_has_no_query_strings_and_no_component_id() {
    let source = Said::new(Ok("ok"));
    let asked = source.asked();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let label = format!("Search https://shop.test/find?token=SECRET123 {}", "y".repeat(400));
    let mut fake = Fake::new(vec![("home", page("/", vec![c(&label, "textbox")]))]);
    let done = run(&mut fake, &opts);
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].label.contains("SECRET123"), "{}", asked[0].label);
    assert!(asked[0].label.chars().count() <= 200);
    let logged = done.model_calls[0].data().to_string();
    assert!(!logged.contains("SECRET123"), "{logged}");
}

#[test]
fn the_same_kind_of_field_is_not_asked_about_twice_in_one_run() {
    // Two pages, each with a box called "Search": the second gets the answer the first got.
    let source = Said::new(Ok("blue pen"));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut fake = Fake::new(vec![
        (
            "home",
            page(
                "/",
                vec![C::link("Other", "other", "/other"), c("Search", "textbox").at("zz")],
            ),
        ),
        ("other", page("/other", vec![c("Search", "textbox")])),
    ]);
    run(&mut fake, &opts);
    assert_eq!(fake.typed, ["blue pen", "blue pen"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn cancel_ends_a_walk_whose_model_call_is_stuck_within_a_second_and_keeps_what_was_found() {
    let source = Said::new(Ok("late")).slow(Duration::from_secs(60));
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut fake = form(&["Search products"]);
    let flag = AtomicBool::new(false);
    let started = Instant::now();
    let done = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        explore(&mut fake, &opts, &flag, &mut |_| {})
    });
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert_eq!(done.graph.stopped, Stop::Cancelled);
    assert_eq!(done.graph.states.len(), 1);
    assert_eq!(outcomes(&done), ["stopped"]);
    assert!(fake.typed.is_empty(), "nothing was typed after the cancel");
}

#[test]
fn a_call_that_takes_too_long_is_given_up_and_the_generator_types() {
    let source = Said::new(Ok("late")).slow(Duration::from_secs(30));
    let mut opts = with_writes();
    use_model(&mut opts, source);
    opts.model.as_mut().unwrap().timeout = Duration::from_millis(300);
    let mut fake = form(&["Search products"]);
    let started = Instant::now();
    let done = run(&mut fake, &opts);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(done.graph.stopped, Stop::Complete);
    assert_eq!(outcomes(&done), ["timed-out"]);
    assert!(fake.typed[0].len() < 20 && fake.typed[0] != "late", "{:?}", fake.typed);
}

#[test]
fn a_model_call_counts_against_the_wall_clock_budget() {
    let source = Said::new(Ok("late")).slow(Duration::from_secs(30));
    let mut opts = with_writes();
    use_model(&mut opts, source);
    opts.budgets.timeout = Duration::from_millis(400);
    let mut fake = form(&["Search products"]);
    let started = Instant::now();
    let done = run(&mut fake, &opts);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(done.graph.stopped, Stop::Timeout);
    assert_eq!(outcomes(&done), ["stopped"]);
}

#[test]
fn a_replay_of_a_run_that_used_a_model_asks_no_model() {
    let labels = ["Search products", "Your email", "Company"];
    let source = Said::new(Ok("blue pen"));
    let calls = source.calls();
    let mut opts = with_writes();
    use_model(&mut opts, source);
    let mut first = form(&labels);
    let done = run(&mut first, &opts);
    assert_eq!(outcomes(&done), ["used", "used", "used"]);
    let made = calls.load(Ordering::SeqCst);
    assert!(made >= 1);

    // What the run wrote to events.jsonl, one line for each call.
    let events: String = done
        .model_calls
        .iter()
        .map(|call| {
            format!(
                "{}\n",
                serde_json::json!({ "type": "model.call", "message": "", "data": call.data() })
            )
        })
        .collect();
    let recorded = Recorded::from_events(&events);

    // The second run is given the record and not the model. The model's counter does not move.
    let mut again = with_writes();
    again.model = Some(ModelSettings {
        source: Arc::new(recorded),
        max_calls: DEFAULT_MAX_MODEL_CALLS,
        timeout: DEFAULT_MODEL_TIMEOUT,
    });
    let mut second = form(&labels);
    let replayed = run(&mut second, &again);
    assert_eq!(calls.load(Ordering::SeqCst), made, "the model was not called");
    assert_eq!(second.typed, first.typed, "the same text was typed");
    assert_eq!(
        replayed.graph.to_bytes(),
        done.graph.to_bytes(),
        "and the graph is the same"
    );
    assert!(replayed.model_calls.iter().all(|call| call.source == "recorded"));
}

#[test]
fn a_record_of_a_run_is_read_back_by_key_and_only_for_calls_that_were_used() {
    let field = |label: &str| Field {
        role: "textbox".to_string(),
        label: label.to_string(),
        ..Field::default()
    };
    let event = |outcome: &str, label: &str, answer: &str| {
        serde_json::json!({
            "type": "model.call",
            "data": { "outcome": outcome, "key": field(label).key(), "answer": answer },
        })
        .to_string()
    };
    let text = [
        r#"{"type":"progress","message":"x"}"#.to_string(),
        event("rejected", "A", "bad"),
        event("used", "B", "good"),
        "not json".to_string(),
    ]
    .join("\n");
    let recorded = Recorded::from_events(&text);
    assert_eq!(recorded.value(&field("B")), Ok("good".to_string()));
    assert!(recorded.value(&field("A")).is_err());
}

#[test]
fn text_in_a_label_cannot_make_two_different_fields_share_a_key() {
    // Joined with a bar, a label "A|B" with no type and a label "A" with the type "B" were the same field.
    let one = Field {
        role: "textbox".to_string(),
        label: "A|B".to_string(),
        ..Field::default()
    };
    let other = Field {
        role: "textbox".to_string(),
        label: "A".to_string(),
        input_type: Some("B".to_string()),
        ..Field::default()
    };
    assert_ne!(one.key(), other.key());
    // Quotes and brackets in a label do not blur the parts either.
    let tricky = Field {
        label: r#"x","y"#.to_string(),
        ..one.clone()
    };
    assert_ne!(
        tricky.key(),
        Field {
            label: "x".to_string(),
            input_type: Some("y".to_string()),
            ..one
        }
        .key()
    );
    // And the same field has the same key each time.
    assert_eq!(other.key(), other.clone().key());
}

#[test]
fn two_walks_without_a_model_with_the_same_seed_write_the_same_bytes_whatever_they_type() {
    let one = run(&mut form(&["Search", "Email", "Note"]), &with_writes())
        .graph
        .to_bytes();
    let two = run(&mut form(&["Search", "Email", "Note"]), &with_writes())
        .graph
        .to_bytes();
    assert_eq!(one, two);
}

// ---------------------------------------------------------------------------------------------------------------
// Events and coverage (EXP-7)
// ---------------------------------------------------------------------------------------------------------------

fn kinds(told: &[Told]) -> Vec<&'static str> {
    told.iter().map(|t| t.kind).collect()
}

#[test]
fn the_end_of_an_action_is_told_in_the_sentence_the_terminal_shows() {
    let (_, told) = run_told(&mut Fake::new(tree()), &options());
    let ends: Vec<&str> = told
        .iter()
        .filter(|t| t.kind == "explore.action.end")
        .map(|t| t.message.as_str())
        .collect();
    assert_eq!(ends[0], r#"e0: click link "A" on s0 -> s1"#);
    assert_eq!(ends.len(), 4);
    let new: Vec<&str> = told
        .iter()
        .filter(|t| t.kind == "explore.state.new")
        .map(|t| t.message.as_str())
        .collect();
    assert_eq!(new[0], format!("s0 is new (depth 0): {ORIGIN}/"));
    assert_eq!(new[1], format!("s1 is new (depth 1): {ORIGIN}/a"));
    // The data a tool reads.
    let first = told.iter().find(|t| t.kind == "explore.action.end").unwrap();
    assert_eq!(first.data["edge"], "e0");
    assert_eq!(first.data["state"], "s0");
    assert_eq!(first.data["to"], "s1");
    assert_eq!(first.data["status"], "ok");
    assert_eq!(first.data["action"], "click");
}

#[test]
fn an_action_starts_before_it_ends_and_a_state_is_new_before_the_action_that_found_it_ends() {
    let (_, told) = run_told(&mut Fake::new(tree()), &options());
    let at = |kind: &str, field: &str, value: &str| {
        told.iter()
            .position(|t| t.kind == kind && t.data[field] == value)
            .unwrap_or_else(|| panic!("no {kind} with {field} {value}"))
    };
    assert_eq!(told[0].kind, "explore.state.new", "the start page is announced first");
    // e0 found s1: its start, then the screen is announced, then the edge ends.
    assert!(at("explore.action.start", "edge", "e0") < at("explore.state.new", "state", "s1"));
    assert!(at("explore.state.new", "state", "s1") < at("explore.action.end", "edge", "e0"));
    // Each action's start names the edge its end will name.
    let start = told.iter().find(|t| t.kind == "explore.action.start").unwrap();
    assert_eq!(start.data["edge"], "e0");
    assert_eq!(start.data["component"], "c_a");
}

#[test]
fn a_replay_is_told_and_its_actions_are_not_edges() {
    let (done, told) = run_told(&mut Fake::new(shop_with_a_dialog()), &options());
    let replays: Vec<&Told> = told.iter().filter(|t| t.kind == "explore.replay").collect();
    assert_eq!(replays.len(), 1);
    assert_eq!(replays[0].data["state"], "s2");
    assert_eq!(replays[0].data["actions"], 1);
    assert_eq!(replays[0].data["reopened"], true);
    assert_eq!(replays[0].message, "s2 reopened by replaying 1 actions");
    // Five actions, four edges: the replayed one makes an `explore.replay` and no `explore.action.end`.
    assert_eq!(done.graph.actions, 5);
    assert_eq!(told.iter().filter(|t| t.kind == "explore.action.end").count(), 4);
}

#[test]
fn a_request_the_browser_refused_is_told_once_with_the_edge_it_belongs_to_before_the_edge_ends() {
    let mut fake = Fake::new(tree());
    fake.refuse = Some(("c_a", "POST", "http://shop.test/api/cart"));
    let (_, told) = run_told(&mut fake, &options());
    let refused: Vec<&Told> = told
        .iter()
        .filter(|t| t.kind == "explore.policy.block" && t.data["kind"] == "request")
        .collect();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].data["edge"], "e0");
    assert_eq!(refused[0].data["method"], "POST");
    assert_eq!(refused[0].data["url"], "http://shop.test/api/cart");
    assert_eq!(refused[0].data["reason"], "blocked_by_policy");
    let at = told.iter().position(|t| std::ptr::eq(t, refused[0])).unwrap();
    let end = told
        .iter()
        .position(|t| t.kind == "explore.action.end" && t.data["edge"] == "e0")
        .unwrap();
    assert!(at < end);
}

#[test]
fn what_the_rules_leave_alone_is_told_with_the_reason_and_once() {
    let (done, told) = run_told(&mut Fake::new(risky()), &options());
    let blocks: Vec<&Told> = told
        .iter()
        .filter(|t| t.kind == "explore.policy.block" && t.data["kind"] == "component")
        .collect();
    assert!(!blocks.is_empty());
    assert_eq!(blocks.len(), done.graph.skipped.len());
    for (told, skipped) in blocks.iter().zip(&done.graph.skipped) {
        assert_eq!(told.data["reason"], skipped.reason.as_str());
        assert_eq!(told.data["component"], skipped.component.as_str());
        assert_eq!(told.data["state"], skipped.state.as_str());
    }
}

#[test]
fn a_model_call_is_told_while_the_walk_goes_on_before_the_action_it_is_for() {
    let mut opts = with_writes();
    use_model(&mut opts, Said::new(Ok("blue pen")));
    let (done, told) = run_told(&mut form(&["Search products"]), &opts);
    assert_eq!(done.model_calls.len(), 1);
    let order = kinds(&told);
    let call = order.iter().position(|k| *k == "model.call").unwrap();
    let start = order.iter().position(|k| *k == "explore.action.start").unwrap();
    assert!(
        call < start,
        "the text is chosen before the field is typed into: {order:?}"
    );
    let told_call = &told[call];
    assert_eq!(told_call.data["outcome"], "used");
    assert_eq!(told_call.data["answer"], "blue pen");
}

#[test]
fn a_walk_that_touches_everything_has_full_coverage() {
    let done = run(&mut Fake::new(tree()), &options());
    let totals = done.coverage.totals();
    assert_eq!(
        (totals.discovered, totals.touched, totals.skipped, totals.untouched),
        (4, 4, 0, 0)
    );
    assert_eq!(totals.per_mille(), 1000);
    assert_eq!(
        done.coverage.summary(),
        "Coverage: 4 of 4 components touched (100.0%), 0 left alone, 0 not reached. Of the 4 it was allowed to touch: 100.0%"
    );
    assert_eq!(
        done.coverage.states[0].discovered,
        ["c_a", "c_b"],
        "headings are not components to act on"
    );
}

#[test]
fn what_the_walk_did_not_get_to_is_untouched_and_the_figures_still_add_up() {
    let mut opts = options();
    opts.budgets.max_actions = 1;
    let done = run(&mut Fake::new(tree()), &opts);
    assert_eq!(done.graph.stopped, Stop::MaxActions);
    let totals = done.coverage.totals();
    assert_eq!(totals.touched, 1);
    assert_eq!(totals.untouched, totals.discovered - 1);
    assert_eq!(done.coverage.states[0].untouched, ["c_b"]);
}

#[test]
fn hidden_controls_are_not_discovered_disabled_ones_are_skipped_and_the_rules_give_the_reasons() {
    let mut fake = Fake::new(vec![(
        "home",
        page(
            "/",
            vec![
                C::link("Ok", "home", "/").at("a"),
                C::link("Hidden", "home", "/").at("b").hidden(),
                C::link("Off", "home", "/").at("c").disabled(),
                c("Subscribe", "button").at("d"),
            ],
        ),
    )]);
    let done = run(&mut fake, &options());
    let state = &done.coverage.states[0];
    assert_eq!(
        state.discovered,
        ["c_off", "c_ok", "c_subscribe"],
        "the hidden one is not there to find"
    );
    assert_eq!(state.touched, ["c_ok"]);
    assert_eq!(
        state.skipped,
        [
            ("c_off".to_string(), "disabled".to_string()),
            ("c_subscribe".to_string(), "read-only".to_string()),
        ]
    );
    assert!(state.untouched.is_empty());
    let reasons: Vec<&str> = done.coverage.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(reasons, ["disabled", "read-only"]);
    assert_eq!(done.coverage.skipped[0].label, r#"link "Off""#);
}

#[test]
fn a_component_on_every_page_is_counted_on_the_first_screen_that_shows_it_and_not_again() {
    let nav = || -> Vec<C> {
        vec![
            C::link("Nav home", "home", "/").at("header"),
            C::link("Nav about", "about", "/about").at("header"),
            C::link("Nav shop", "shop", "/shop").at("header"),
        ]
    };
    let mut home = nav();
    home.push(C::link("Deals", "deals", "/deals").at("main"));
    let mut about = nav();
    about.push(c("About title", "heading"));
    let mut shop = nav();
    shop.push(c("Shop title", "heading"));
    let mut deals = nav();
    deals.push(c("Deals title", "heading"));
    let mut opts = options();
    opts.chrome_pages = 3;
    let mut fake = Fake::new(vec![
        ("home", page("/", home)),
        ("about", page("/about", about)),
        ("shop", page("/shop", shop)),
        ("deals", page("/deals", deals)),
    ]);
    let done = run(&mut fake, &opts);
    assert_eq!(done.coverage.shared, done.graph.chrome);
    assert_eq!(done.coverage.shared.len(), 3);
    assert_eq!(
        done.coverage.states[0].discovered.len(),
        4,
        "the header and the link to Deals"
    );
    for later in &done.coverage.states[1..] {
        assert!(later.discovered.is_empty(), "{}: {:?}", later.state, later.discovered);
    }
    assert_eq!(done.coverage.totals().discovered, 4);
}

#[test]
fn the_coverage_of_a_walk_is_the_same_bytes_each_time() {
    let one = run(&mut Fake::new(shop_with_a_dialog()), &options())
        .coverage
        .to_bytes();
    let two = run(&mut Fake::new(shop_with_a_dialog()), &options())
        .coverage
        .to_bytes();
    assert_eq!(one, two);
}

#[test]
fn a_replay_that_fails_is_told_too_with_the_actions_it_cost() {
    // The dialog comes the first time only: the replays click "Open" and find nothing. Two attempts, one action each.
    struct Once(Fake, u32);
    impl Browser for Once {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.0.load(url)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            if i.selector == "sel:c_open" {
                self.1 += 1;
                if self.1 > 1 {
                    return Ok(Acted {
                        effect: Effect::default(),
                        view: self.0.view(),
                    });
                }
            }
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut browser = Once(Fake::new(shop_with_a_dialog()), 0);
    static NEVER: AtomicBool = AtomicBool::new(false);
    let mut told = Vec::new();
    let done = explore(&mut browser, &options(), &NEVER, &mut |event| {
        told.push(Told {
            kind: event.kind(),
            message: event.message(),
            data: event.data(),
        });
    });
    check_walk(&done, &told);
    let replays: Vec<&Told> = told.iter().filter(|t| t.kind == "explore.replay").collect();
    assert_eq!(replays.len(), 2, "two attempts");
    assert!(replays
        .iter()
        .all(|t| t.data["reopened"] == false && t.data["actions"] == 1));
    assert_eq!(
        replays[0].message,
        "s2 was not brought back by replaying (1 actions made)"
    );
    assert_eq!(done.graph.unreachable, ["s2"]);
}

// ---------------------------------------------------------------------------------------------------------------
// Time limits for the parts of a walk that can use all of it
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn reading_the_pages_that_show_what_is_on_every_page_stops_when_it_uses_a_fifth_of_the_time() {
    let site = || {
        Fake::new(vec![
            (
                "home",
                page(
                    "/",
                    vec![
                        C::link("One", "one", "/one"),
                        C::link("Two", "two", "/two"),
                        C::link("Three", "three", "/three"),
                    ],
                ),
            ),
            ("one", leaf("/one", "One")),
            ("two", leaf("/two", "Two")),
            ("three", leaf("/three", "Three")),
        ])
    };
    let mut opts = options();
    opts.chrome_pages = 5;
    opts.budgets.max_actions = 0;
    // On a site where a load takes 250 ms and the walk has 2 s, a fifth is 400 ms: the start page and one more.
    opts.budgets.timeout = Duration::from_secs(2);
    let mut fake = site();
    fake.load_delay = Duration::from_millis(250);
    let mut lines = Vec::new();
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(&mut fake, &opts, &NEVER, &mut |e| lines.push(e.message()));
    assert_eq!(
        done.graph.chrome_pages, 2,
        "the start page and one more, not the three there were"
    );
    assert!(
        lines.iter().any(|l| l.contains("using too much of the time")),
        "{lines:?}"
    );
    // On a fast site all of them are read.
    let mut fast = site();
    let mut opts = options();
    opts.chrome_pages = 5;
    opts.budgets.max_actions = 0;
    assert_eq!(run(&mut fast, &opts).graph.chrome_pages, 4);
}

#[test]
fn replays_that_use_a_quarter_of_the_time_end_the_trying_and_the_screen_is_given_up_with_the_reason() {
    // The click that opens the dialog is slow the second time (a slow site) and then does nothing: the replay fails
    // after 800 ms. A quarter of 3 s is 750 ms, so there is no second attempt.
    struct SlowReplay(Fake, u32);
    impl Browser for SlowReplay {
        fn load(&mut self, url: &str) -> Result<View, StepError> {
            self.0.load(url)
        }
        fn act(&mut self, i: &ViewInstance, a: &Act, t: u64) -> Result<Acted, StepError> {
            if i.selector == "sel:c_open" {
                self.1 += 1;
                if self.1 > 1 {
                    std::thread::sleep(Duration::from_millis(800));
                    return Ok(Acted {
                        effect: Effect::default(),
                        view: self.0.view(),
                    });
                }
            }
            self.0.act(i, a, t)
        }
        fn requests(&self) -> Option<BTreeMap<String, u64>> {
            self.0.requests()
        }
    }
    let mut browser = SlowReplay(Fake::new(shop_with_a_dialog()), 0);
    let mut opts = options();
    opts.budgets.timeout = Duration::from_secs(3);
    static NEVER: AtomicBool = AtomicBool::new(false);
    let mut told = Vec::new();
    let done = explore(&mut browser, &opts, &NEVER, &mut |event| {
        told.push(Told {
            kind: event.kind(),
            message: event.message(),
            data: event.data(),
        });
    });
    check_walk(&done, &told);
    let replays = told.iter().filter(|t| t.kind == "explore.replay").count();
    assert_eq!(replays, 1, "one attempt, and the time set aside was gone for a second");
    assert!(told
        .iter()
        .any(|t| t.kind == "progress" && t.message.contains("the time set aside for replays is used up")));
    assert_eq!(done.graph.unreachable, ["s2"]);
    assert_eq!(done.graph.stopped, Stop::Complete);
}

// ---------------------------------------------------------------------------------------------------------------
// Replay (EXP-8): a graph a walk wrote, performed again against a site in memory
// ---------------------------------------------------------------------------------------------------------------

/// The graph of a walk, read back the way `traceqa replay` reads it: from its bytes.
fn doc_of(done: &Explored) -> Doc {
    Doc::parse(std::str::from_utf8(&done.graph.to_bytes()).unwrap()).unwrap()
}

/// Walks `pages` with the rules given, and reads the graph back.
fn walked(pages: Vec<(&'static str, P)>, writes: bool) -> Doc {
    let mut opts = options();
    opts.rules = Rules::new(&opts.start_url, writes, &[]).unwrap();
    doc_of(&run(&mut Fake::new(pages), &opts))
}

/// Gives every page thirty more things to look at, so that a page which loses one control is still the same screen by
/// the rule of ninety in a hundred. (A page of three components that loses one is a different screen, and a replay says so.)
fn padded(mut pages: Vec<(&'static str, P)>) -> Vec<(&'static str, P)> {
    for (key, page) in &mut pages {
        for n in 0..30 {
            page.comps.push(c(&format!("Filler {key} {n}"), "heading"));
        }
    }
    pages
}

fn replay_options(writes: bool) -> ReplayOptions {
    ReplayOptions {
        rules: Rules::new(&format!("{ORIGIN}/"), writes, &[]).unwrap(),
        action_timeout: Duration::from_secs(10),
        timeout: Duration::from_secs(600),
        max_requests_per_origin: 2000,
    }
}

/// What a replay told its listener, and what it came to. Every replay of this file is checked by [`check_replay`].
fn replayed_told(fake: &mut Fake, doc: &Doc, mode: &Mode, opts: &ReplayOptions) -> (Replayed, Vec<Told>) {
    static NEVER: AtomicBool = AtomicBool::new(false);
    let mut told = Vec::new();
    let done = replay(fake, doc, mode, opts, &NEVER, &mut |event| {
        told.push(Told {
            kind: event.kind(),
            message: event.message(),
            data: event.data(),
        });
    });
    check_replay(doc, mode, &done, &told);
    (done, told)
}

fn replayed(fake: &mut Fake, doc: &Doc, mode: &Mode, opts: &ReplayOptions) -> Replayed {
    replayed_told(fake, doc, mode, opts).0
}

/// What must hold for any replay, whatever the site.
fn check_replay(doc: &Doc, mode: &Mode, done: &Replayed, told: &[Told]) {
    let asked: Vec<usize> = match mode {
        Mode::Chain(edges) => edges.clone(),
        Mode::All => (0..doc.edges.len()).collect(),
    };
    // One step for each edge asked for, in the order asked, and one event for each step, in the same order.
    let got: Vec<usize> = done.steps.iter().map(|s| s.edge).collect();
    assert_eq!(got, asked, "a step for each edge asked for");
    let events: Vec<&str> = told
        .iter()
        .filter(|t| t.kind == "replay.step")
        .map(|t| t.data["edge"].as_str().unwrap())
        .collect();
    let ids: Vec<&str> = asked.iter().map(|&e| doc.edges[e].id.as_str()).collect();
    assert_eq!(events, ids, "one replay.step for each step");
    for (step, event) in done.steps.iter().zip(told.iter().filter(|t| t.kind == "replay.step")) {
        assert_eq!(event.data["outcome"], step.outcome.as_str());
    }
    // The figures are the sums, and the file says the same as the steps.
    let totals = done.totals();
    assert_eq!(
        totals.reproduced + totals.diverged + totals.missing + totals.blocked + totals.not_run + totals.recorded_failed,
        done.steps.len()
    );
    let file = done.to_json(doc, mode);
    assert_eq!(file["steps"].as_array().unwrap().len(), done.steps.len());
    assert_eq!(file["totals"]["reproduced"], totals.reproduced);
    assert_eq!(done.to_bytes(doc, mode), done.clone().to_bytes(doc, mode));
    // Every break in a chain is followed only by steps that were not run.
    if matches!(mode, Mode::Chain(_)) {
        if let Some(first) = done.steps.iter().position(|s| s.outcome != Outcome::Reproduced) {
            assert!(
                done.steps[first + 1..].iter().all(|s| s.outcome == Outcome::NotRun),
                "{:?}",
                done.steps
            );
        }
    }
    // An action is made for a step that ran, and the replay says how many it made.
    assert!(done.actions as usize >= done.steps.iter().filter(|s| s.outcome == Outcome::Reproduced).count());
}

fn replay_outcomes(done: &Replayed) -> Vec<Outcome> {
    done.steps.iter().map(|s| s.outcome).collect()
}

#[test]
fn replaying_every_edge_of_a_walk_of_the_same_site_reproduces_every_one() {
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(done.ended, Ended::Complete);
    assert_eq!(done.steps.len(), 4);
    assert!(
        replay_outcomes(&done).iter().all(|o| *o == Outcome::Reproduced),
        "{:?}",
        done.steps
    );
    assert!(done.succeeded());
    assert_eq!(done.totals().per_mille(), Some(1000));
    // The same actions the walk made, in the same order.
    assert_eq!(fake.acted, ["c_a", "c_b", "c_a1", "c_b1"]);
}

#[test]
fn a_chain_is_performed_in_one_session_from_the_start_page() {
    let doc = walked(tree(), false);
    let chain = chain_of(&doc, &["e0".to_string(), "e2".to_string()]).unwrap();
    assert_eq!(chain, [0, 2]);
    let mut fake = Fake::new(tree());
    let done = replayed(&mut fake, &doc, &Mode::Chain(chain), &replay_options(false));
    assert_eq!(replay_outcomes(&done), [Outcome::Reproduced, Outcome::Reproduced]);
    assert_eq!(fake.acted, ["c_a", "c_a1"]);
    assert_eq!(
        fake.loaded,
        [format!("{ORIGIN}/")],
        "the start page is opened once and the chain goes on from it"
    );
    assert_eq!(done.steps[1].actual.as_deref(), Some("s3"));
}

#[test]
fn a_chain_ends_at_the_first_step_that_does_not_lead_where_it_did_and_the_rest_are_not_run() {
    let doc = walked(padded(chain(3)), false);
    let all = ["e0", "e1", "e2"].map(String::from);
    let mut fake = Fake::new(padded(chain(3)));
    // The second page has lost its link.
    fake.pages.get_mut("p2").unwrap().comps.retain(|c| c.role != "link");
    let done = replayed(
        &mut fake,
        &doc,
        &Mode::Chain(chain_of(&doc, &all).unwrap()),
        &replay_options(false),
    );
    assert_eq!(
        replay_outcomes(&done),
        [Outcome::Reproduced, Outcome::Reproduced, Outcome::Missing]
    );

    let mut fake = Fake::new(padded(chain(3)));
    fake.pages.get_mut("p1").unwrap().comps.retain(|c| c.role != "link");
    let done = replayed(
        &mut fake,
        &doc,
        &Mode::Chain(chain_of(&doc, &all).unwrap()),
        &replay_options(false),
    );
    assert_eq!(
        replay_outcomes(&done),
        [Outcome::Reproduced, Outcome::Missing, Outcome::NotRun]
    );
    assert_eq!(done.steps[2].note.as_deref(), Some("e1 did not reproduce"));
    assert!(!done.succeeded());
    assert_eq!(fake.acted, ["c_next_0"], "nothing was done after the break");
}

#[test]
fn a_missing_component_is_reported_and_nothing_is_done_to_the_page() {
    let doc = walked(padded(tree()), false);
    let mut fake = Fake::new(padded(tree()));
    fake.pages.get_mut("home").unwrap().comps.retain(|c| c.name != "B");
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(
        replay_outcomes(&done),
        [
            Outcome::Reproduced,
            Outcome::Missing,
            Outcome::Reproduced,
            Outcome::Reproduced
        ]
    );
    assert!(!fake.acted.contains(&"c_b".to_string()));
    assert_eq!(done.totals().missing, 1);
    assert_eq!(done.totals().per_mille(), Some(750));
    assert!(!done.succeeded());
}

#[test]
fn a_page_that_is_another_screen_is_reported_as_diverged_with_the_screen_it_is() {
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    // "A" now leads to the page "B" leads to.
    fake.pages
        .get_mut("home")
        .unwrap()
        .comps
        .iter_mut()
        .find(|c| c.name == "A")
        .unwrap()
        .to = Some("b");
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    let first = &done.steps[0];
    assert_eq!(first.outcome, Outcome::Diverged);
    assert_eq!(first.actual.as_deref(), Some("s2"));
    assert!(first.note.as_deref().unwrap().contains("s2") && first.note.as_deref().unwrap().contains("s1"));

    // A page the graph has no screen like is `unknown`.
    let mut fake = Fake::new(tree());
    fake.pages.insert("new", leaf("/new", "A page nobody saw"));
    fake.pages
        .get_mut("home")
        .unwrap()
        .comps
        .iter_mut()
        .find(|c| c.name == "A")
        .unwrap()
        .to = Some("new");
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(done.steps[0].outcome, Outcome::Diverged);
    assert_eq!(done.steps[0].actual.as_deref(), Some("unknown"));
}

#[test]
fn an_action_that_does_not_complete_is_diverged_and_says_why() {
    let doc = walked(tree(), false);
    for error in [StepError::Failed("boom".into()), StepError::Timeout("slow".into())] {
        let mut fake = Fake::new(tree());
        fake.pages
            .get_mut("home")
            .unwrap()
            .comps
            .iter_mut()
            .find(|c| c.name == "A")
            .unwrap()
            .fails = Some(error);
        let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
        assert_eq!(done.steps[0].outcome, Outcome::Diverged);
        assert_eq!(done.steps[0].actual, None);
        assert!(
            done.steps[0].note.as_deref().unwrap().contains("did not complete"),
            "{:?}",
            done.steps[0]
        );
    }
}

#[test]
fn a_click_that_is_blocked_is_tried_once_with_the_key_as_the_walk_did() {
    let blocked = || {
        let mut pages = tree();
        let home = &mut pages[0].1;
        home.comps.iter_mut().find(|c| c.name == "A").unwrap().click_blocked = true;
        pages
    };
    let doc = walked(blocked(), false);
    assert_eq!(doc.edges[0].action, ActionKind::Press);
    let mut fake = Fake::new(blocked());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert!(
        replay_outcomes(&done).iter().all(|o| *o == Outcome::Reproduced),
        "{:?}",
        done.steps
    );
    assert_eq!(fake.keys, ["Enter"]);
}

#[test]
fn an_edge_the_walk_did_not_complete_is_not_counted_against_the_replay() {
    let failing = || {
        let mut pages = tree();
        pages[0].1.comps.iter_mut().find(|c| c.name == "B").unwrap().fails = Some(StepError::Failed("no".into()));
        pages
    };
    let doc = walked(failing(), false);
    assert_eq!(doc.edges[1].status, Status::Failed);
    let mut fake = Fake::new(failing());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(done.steps[1].outcome, Outcome::RecordedFailed);
    assert_eq!(done.totals().replayable, done.steps.len() - 1);
    assert!(done.succeeded());
    assert!(
        !fake.acted.contains(&"c_b".to_string()),
        "a failed edge is not done again"
    );
    assert_eq!(done.totals().per_mille(), Some(1000));

    // A chain may not name one: it leads nowhere.
    let why = chain_of(&doc, &["e1".to_string()]).err().unwrap().0;
    assert!(why.contains("failed"), "{why}");
}

/// home has two text boxes and a link.
fn text_form() -> Vec<(&'static str, P)> {
    vec![
        (
            "home",
            page(
                "/",
                vec![
                    c("City", "textbox"),
                    c("Company", "textbox"),
                    C::link("About", "about", "/about"),
                ],
            ),
        ),
        ("about", leaf("/about", "About title")),
    ]
}

#[test]
fn what_was_typed_is_typed_again_when_writes_are_allowed_and_left_alone_when_they_are_not() {
    let doc = walked(text_form(), true);
    let typed: Vec<&str> = doc.edges.iter().filter_map(|e| e.value.as_deref()).collect();
    assert_eq!(typed.len(), 2, "{:?}", doc.edges);

    let mut fake = Fake::new(text_form());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(true));
    assert!(done.succeeded(), "{:?}", done.steps);
    assert_eq!(fake.typed, typed, "the same text, in the same order");

    // Read-only: nothing is typed, the edges are blocked, and the run does not pass.
    let mut fake = Fake::new(text_form());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert!(fake.typed.is_empty());
    let blocked: Vec<_> = done.steps.iter().filter(|s| s.outcome == Outcome::Blocked).collect();
    assert_eq!(blocked.len(), 2);
    assert!(blocked[0].note.as_deref().unwrap().contains("read-only"));
    assert!(!done.succeeded());
}

#[test]
fn a_destructive_name_is_blocked_even_with_writes_allowed() {
    let doc = walked(text_form(), true);
    let mut opts = replay_options(true);
    opts.rules = Rules::new(&format!("{ORIGIN}/"), true, &["city".to_string()]).unwrap();
    let mut fake = Fake::new(text_form());
    let done = replayed(&mut fake, &doc, &Mode::All, &opts);
    let city = done
        .steps
        .iter()
        .find(|s| doc.edges[s.edge].label.contains("City"))
        .unwrap();
    assert_eq!(city.outcome, Outcome::Blocked);
    assert!(city.note.as_deref().unwrap().contains("destructive"));
    assert_eq!(fake.typed.len(), 1, "only the other box was typed into");
}

#[test]
fn a_graph_of_the_first_layout_has_no_text_to_type_and_the_edge_is_blocked_and_says_so() {
    let mut doc = walked(text_form(), true);
    for edge in &mut doc.edges {
        edge.value = None;
    }
    let mut fake = Fake::new(text_form());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(true));
    assert!(fake.typed.is_empty(), "nothing is guessed");
    let blocked: Vec<_> = done.steps.iter().filter(|s| s.outcome == Outcome::Blocked).collect();
    assert_eq!(blocked.len(), 2);
    assert!(blocked[0].note.as_deref().unwrap().contains("no value"));
}

#[test]
fn edges_on_one_screen_that_changed_nothing_are_done_one_after_another_without_opening_the_page_again() {
    let doc = walked(text_form(), true);
    let mut fake = Fake::new(text_form());
    replayed(&mut fake, &doc, &Mode::All, &replay_options(true));
    // Two typed edges on the start page share one load; the link then needs the page again.
    assert_eq!(fake.loaded.len(), 2, "{:?}", fake.loaded);
}

#[test]
fn a_screen_with_no_address_is_reopened_by_performing_the_way_to_it_and_this_is_told() {
    let doc = walked(shop_with_a_dialog(), false);
    let mut fake = Fake::new(shop_with_a_dialog());
    let (done, told) = replayed_told(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert!(done.succeeded(), "{:?}", done.steps);
    let reopened: Vec<&Told> = told.iter().filter(|t| t.kind == "replay.reopen").collect();
    assert_eq!(reopened.len(), 2, "one for each edge made on the dialog");
    let dialog = doc.states.iter().find(|s| s.modal.is_some()).unwrap().id.clone();
    assert!(reopened
        .iter()
        .all(|t| t.data["reopened"] == true && t.data["state"] == dialog.as_str()));
    // 4 edges, and a replayed "Open" for each of the two made on the dialog.
    assert_eq!(done.actions, 6);
    // The dialog's address is tried twice, once.
    assert_eq!(fake.acted.iter().filter(|a| *a == "c_open").count(), 3);
}

#[test]
fn a_screen_that_cannot_be_reopened_is_tried_once_and_its_edges_are_not_run() {
    let doc = walked(padded(shop_with_a_dialog()), false);
    let mut fake = Fake::new(padded(shop_with_a_dialog()));
    // The link that opens the dialog is gone.
    fake.pages.get_mut("home").unwrap().comps.retain(|c| c.name != "Open");
    let (done, told) = replayed_told(&mut fake, &doc, &Mode::All, &replay_options(false));
    let by_edge: Vec<(&str, Outcome)> = done
        .steps
        .iter()
        .map(|s| (doc.edges[s.edge].label.as_str(), s.outcome))
        .collect();
    assert!(by_edge.contains(&("link \"Open\"", Outcome::Missing)), "{by_edge:?}");
    let not_run: Vec<_> = done.steps.iter().filter(|s| s.outcome == Outcome::NotRun).collect();
    assert_eq!(not_run.len(), 2);
    assert!(not_run[0]
        .note
        .as_deref()
        .unwrap()
        .contains("could not be opened again"));
    // Two attempts to reopen at most, for the first edge; the second edge does not try again.
    assert!(told.iter().filter(|t| t.kind == "replay.reopen").count() <= 2);
}

#[test]
fn the_way_to_a_screen_is_the_fewest_edges_and_never_one_that_typed_text() {
    let doc = walked(chain(3), false);
    assert_eq!(shortest_way(&doc, 3), Some(vec![0, 1, 2]));
    assert_eq!(shortest_way(&doc, 0), Some(vec![]));
    assert_eq!(shortest_way(&doc, 1), Some(vec![0]));

    // A way that needs a typed edge is no way.
    let mut typed = walked(tree(), false);
    typed.edges[0].action = ActionKind::Type;
    assert_eq!(shortest_way(&typed, 1), None);
    assert_eq!(shortest_way(&typed, 2), Some(vec![1]));

    // Edges the walk did not complete lead nowhere.
    let mut failed = walked(tree(), false);
    failed.edges[0].status = Status::Failed;
    assert_eq!(shortest_way(&failed, 1), None);
}

#[test]
fn a_chain_must_follow_on_from_the_start_page_and_each_edge_from_the_one_before() {
    let doc = walked(tree(), false);
    let names = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert!(chain_of(&doc, &names(&["e0", "e2"])).is_ok());
    let apart = chain_of(&doc, &names(&["e0", "e1"])).err().unwrap().0;
    assert!(apart.contains("e1") && apart.contains("s1"), "{apart}");
    let not_start = chain_of(&doc, &names(&["e2"])).err().unwrap().0;
    assert!(not_start.contains("e2"), "{not_start}");
    assert!(chain_of(&doc, &names(&["e9"])).err().unwrap().0.contains("e9"));
    assert!(chain_of(&doc, &[]).is_err());
}

#[test]
fn a_start_page_that_is_not_the_first_screen_ends_a_chain_at_once() {
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    fake.pages.get_mut("home").unwrap().comps = vec![c("Something else", "heading"), C::link("Z", "a", "/a")];
    let done = replayed(&mut fake, &doc, &Mode::Chain(vec![0, 2]), &replay_options(false));
    assert_eq!(replay_outcomes(&done), [Outcome::Diverged, Outcome::NotRun]);
    assert!(done.steps[0].note.as_deref().unwrap().contains("start page"));
    assert!(fake.acted.is_empty());
}

#[test]
fn cancel_ends_the_replay_and_every_edge_asked_for_still_has_a_step() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    fake.cancel = Some((2, &CANCEL));
    let mut told = Vec::new();
    let done = replay(
        &mut fake,
        &doc,
        &Mode::All,
        &replay_options(false),
        &CANCEL,
        &mut |event| {
            told.push(Told {
                kind: event.kind(),
                message: event.message(),
                data: event.data(),
            })
        },
    );
    check_replay(&doc, &Mode::All, &done, &told);
    assert_eq!(done.ended, Ended::Cancelled);
    assert_eq!(done.steps.len(), 4);
    assert!(done.steps[2..].iter().all(|s| s.outcome == Outcome::NotRun));
    assert!(!done.succeeded());
}

#[test]
fn the_clock_ends_the_replay_and_the_edges_not_reached_were_not_run() {
    let doc = walked(tree(), false);
    let mut opts = replay_options(false);
    opts.timeout = Duration::ZERO;
    let mut fake = Fake::new(tree());
    let done = replayed(&mut fake, &doc, &Mode::All, &opts);
    assert_eq!(done.ended, Ended::Timeout);
    assert!(replay_outcomes(&done).iter().all(|o| *o == Outcome::NotRun));
    assert!(fake.acted.is_empty() && fake.loaded.is_empty());
}

#[test]
fn the_request_budget_ends_the_replay() {
    let doc = walked(tree(), false);
    let mut opts = replay_options(false);
    opts.max_requests_per_origin = 3;
    let mut fake = Fake::new(tree());
    let done = replayed(&mut fake, &doc, &Mode::All, &opts);
    assert_eq!(done.ended, Ended::MaxRequests);
    assert!(done.steps.iter().any(|s| s.outcome == Outcome::NotRun));
    assert!(!done.succeeded());
}

#[test]
fn a_browser_that_is_lost_ends_the_replay_with_the_error_and_every_edge_has_a_step() {
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    fake.stranded = true;
    // Nothing can be acted on until a load works, and the first page to open is not there.
    fake.pages.get_mut("home").unwrap().url = format!("{ORIGIN}/moved");
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert!(!done.succeeded());
    assert_eq!(done.steps.len(), 4);
}

#[test]
fn a_graph_the_screens_cannot_be_built_from_gives_an_error_and_a_step_for_every_edge() {
    let mut doc = walked(tree(), false);
    // Two screens that are alike.
    doc.states[2] = doc.states[1].clone();
    doc.states[2].id = "s2".to_string();
    let mut fake = Fake::new(tree());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(done.ended, Ended::Error);
    assert!(done.error.as_deref().unwrap().contains("alike"));
    assert!(replay_outcomes(&done).iter().all(|o| *o == Outcome::NotRun));
}

#[test]
fn a_secret_in_an_address_stays_hidden_in_the_graph_and_the_page_is_still_recognised() {
    let secret = || {
        vec![
            (
                "home",
                page("/", vec![C::link("Account", "acct", "/acct?token=hunter2")]),
            ),
            ("acct", {
                let mut p = leaf("/acct", "Account title");
                p.url = format!("{ORIGIN}/acct?token=hunter2");
                p
            }),
        ]
    };
    let doc = walked(secret(), false);
    assert!(
        doc.states[1].url.contains("REDACTED") && !doc.states[1].url.contains("hunter2"),
        "{}",
        doc.states[1].url
    );

    // A chain gets there by acting, so the hidden address is no obstacle.
    let mut fake = Fake::new(secret());
    let done = replayed(&mut fake, &doc, &Mode::Chain(vec![0]), &replay_options(false));
    assert_eq!(replay_outcomes(&done), [Outcome::Reproduced]);

    // The whole graph reopens it the same way, and the address with the hidden value is never opened.
    let mut fake = Fake::new(secret());
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert!(done.succeeded());
    assert!(fake.loaded.iter().all(|u| !u.contains("REDACTED")), "{:?}", fake.loaded);
}

#[test]
fn the_same_replay_writes_the_same_bytes() {
    let doc = walked(tree(), false);
    let bytes = |fake: &mut Fake| replayed(fake, &doc, &Mode::All, &replay_options(false)).to_bytes(&doc, &Mode::All);
    assert_eq!(bytes(&mut Fake::new(tree())), bytes(&mut Fake::new(tree())));
}

#[test]
fn the_file_has_the_counts_the_mode_and_a_line_for_each_step() {
    let doc = walked(padded(tree()), false);
    let mut fake = Fake::new(padded(tree()));
    fake.pages.get_mut("home").unwrap().comps.retain(|c| c.name != "B");
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    let file = done.to_json(&doc, &Mode::All);
    assert_eq!(file["schemaVersion"], 1);
    assert_eq!(file["mode"], "all");
    assert_eq!(file["ended"], "complete");
    assert_eq!(file["graph"]["edges"], 4);
    assert_eq!(file["totals"]["replayable"], 4);
    assert_eq!(file["totals"]["missing"], 1);
    assert_eq!(file["totals"]["reproducedPerMille"], 750);
    let second = &file["steps"][1];
    assert_eq!(second["edge"], "e1");
    assert_eq!(second["outcome"], "missing");
    assert_eq!(second["expected"], "s2");
    assert!(second.get("actual").is_none());
    let none = Replayed {
        steps: vec![],
        ended: Ended::Complete,
        actions: 0,
        error: None,
    }
    .to_json(&doc, &Mode::Chain(vec![]));
    assert!(none["totals"].get("reproducedPerMille").is_none());
}

#[test]
fn the_person_watching_is_told_each_step_in_a_sentence() {
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    let (_, told) = replayed_told(&mut fake, &doc, &Mode::Chain(vec![0]), &replay_options(false));
    let step = told.iter().find(|t| t.kind == "replay.step").unwrap();
    assert_eq!(step.message, "e0: click link \"A\" on s0 -> reproduced");
    assert_eq!(step.data["expected"], "s1");
    assert_eq!(step.data["actual"], "s1");
}

#[test]
fn a_chain_stops_at_a_step_that_led_to_another_page_though_the_page_is_there() {
    let doc = walked(padded(chain(3)), false);
    let all = ["e0", "e1", "e2"].map(String::from);
    let mut fake = Fake::new(padded(chain(3)));
    // The link on the second page now leads to the last page.
    fake.pages
        .get_mut("p1")
        .unwrap()
        .comps
        .iter_mut()
        .find(|c| c.role == "link")
        .unwrap()
        .to = Some("p3");
    let done = replayed(
        &mut fake,
        &doc,
        &Mode::Chain(chain_of(&doc, &all).unwrap()),
        &replay_options(false),
    );
    assert_eq!(
        replay_outcomes(&done),
        [Outcome::Reproduced, Outcome::Diverged, Outcome::NotRun]
    );
    assert_eq!(fake.acted.len(), 2, "nothing was done after the step that diverged");
}

#[test]
fn a_way_that_leads_to_another_page_does_not_bring_a_screen_back() {
    let doc = walked(padded(shop_with_a_dialog()), false);
    let mut fake = Fake::new(padded(shop_with_a_dialog()));
    // "Open" now leads to the about page.
    fake.pages
        .get_mut("home")
        .unwrap()
        .comps
        .iter_mut()
        .find(|c| c.name == "Open")
        .unwrap()
        .to = Some("about");
    let (done, told) = replayed_told(&mut fake, &doc, &Mode::All, &replay_options(false));
    let reopens: Vec<&Told> = told.iter().filter(|t| t.kind == "replay.reopen").collect();
    assert!(!reopens.is_empty());
    assert!(reopens.iter().all(|t| t.data["reopened"] == false), "{reopens:?}");
    assert!(
        done.steps.iter().filter(|s| s.outcome == Outcome::NotRun).count() >= 2,
        "{:?}",
        done.steps
    );
    // What was told on the way: the screen could not be brought back.
    assert!(
        told.iter()
            .any(|t| t.kind == "progress" && t.message.contains("did not bring it back")),
        "{told:?}"
    );
}

#[test]
fn a_reopening_tells_how_many_actions_it_made() {
    let doc = walked(shop_with_a_dialog(), false);
    let mut fake = Fake::new(shop_with_a_dialog());
    let (_, told) = replayed_told(&mut fake, &doc, &Mode::All, &replay_options(false));
    let reopened: Vec<&Told> = told.iter().filter(|t| t.kind == "replay.reopen").collect();
    assert!(!reopened.is_empty());
    assert!(reopened.iter().all(|t| t.data["actions"] == 1), "{reopened:?}");
    assert!(
        reopened[0].message.contains("by performing 1 actions"),
        "{}",
        reopened[0].message
    );
}

#[test]
fn a_missing_component_is_not_counted_as_an_action() {
    let doc = walked(padded(tree()), false);
    let mut fake = Fake::new(padded(tree()));
    fake.pages.get_mut("home").unwrap().comps.retain(|c| c.name != "B");
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(done.actions as usize, fake.acted.len());
    assert_eq!(done.actions, 3);
}

#[test]
fn a_screen_with_a_hidden_part_in_its_address_is_reopened_by_its_way_and_the_address_is_not_opened() {
    let pages = || {
        vec![
            (
                "home",
                page(
                    "/",
                    vec![
                        C::link("Account", "acct", "/acct?token=hunter2"),
                        C::link("Other", "other", "/other"),
                    ],
                ),
            ),
            ("acct", {
                let mut p = page(
                    "/acct",
                    vec![c("Account title", "heading"), C::link("Deeper", "deep", "/deep")],
                );
                p.url = format!("{ORIGIN}/acct?token=hunter2");
                p
            }),
            ("other", leaf("/other", "Other title")),
            ("deep", leaf("/deep", "Deep title")),
        ]
    };
    let doc = walked(pages(), false);
    assert!(doc.states.iter().any(|s| s.url.contains("REDACTED")));
    let mut fake = Fake::new(pages());
    let (done, told) = replayed_told(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert!(done.succeeded(), "{:?}", done.steps);
    assert!(fake.loaded.iter().all(|u| !u.contains("REDACTED")), "{:?}", fake.loaded);
    assert!(
        told.iter().any(|t| t.kind == "replay.reopen"),
        "it was reopened by performing its way"
    );
}

#[test]
fn a_control_the_browser_cannot_find_when_it_acts_is_missing_and_is_not_counted_as_an_action() {
    let doc = walked(tree(), false);
    let mut fake = Fake::new(tree());
    fake.pages
        .get_mut("home")
        .unwrap()
        .comps
        .iter_mut()
        .find(|c| c.name == "A")
        .unwrap()
        .fails = Some(StepError::NotFound("gone".into()));
    let done = replayed(&mut fake, &doc, &Mode::All, &replay_options(false));
    assert_eq!(done.steps[0].outcome, Outcome::Missing);
    assert_eq!(
        done.actions as usize,
        fake.acted.len() - 1,
        "the try that found nothing made no action"
    );
}
