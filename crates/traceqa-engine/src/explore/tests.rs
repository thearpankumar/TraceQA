//! The rules of the walk, against a small site that lives in memory. Whether the real browser behaves is not tested
//! here: `tests/test_explore.py` runs the real command in Chromium, Firefox and WebKit.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
}

impl Fake {
    fn new(pages: Vec<(&'static str, P)>) -> Self {
        Self {
            pages: pages.into_iter().collect(),
            current: "home",
            acted: Vec::new(),
            kinds: Vec::new(),
            keys: Vec::new(),
            stranded: false,
            loaded: Vec::new(),
            requests: 0,
            per_request: 1,
            count_requests: true,
            delay: Duration::ZERO,
            cancel: None,
            reverse: false,
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
    }
}

fn run(fake: &mut Fake, options: &Options) -> Explored {
    static NEVER: AtomicBool = AtomicBool::new(false);
    let done = explore(fake, options, &NEVER, &mut |_| {});
    // Whatever the walk was about, its parts agree: one action for each edge that was made, and the actions of the
    // replays that reopened a screen, which make no edge.
    let made = done.graph.edges.iter().filter(|e| e.status != Status::Missing).count();
    assert!(done.graph.actions as usize >= made, "{:?}", done.graph.edges);
    done
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
    let done = explore(&mut fake, &options(), &NEVER, &mut |line| lines.push(line.to_string()));
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
