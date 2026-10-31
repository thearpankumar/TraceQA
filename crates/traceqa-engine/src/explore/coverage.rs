//! `coverage.json`: how much of what a walk found it touched (backlog EXP-7).
//!
//! It is worked out from the same data as `graph.json`, at the end of the walk, so the two cannot disagree and it
//! costs the browser nothing.
//!
//! **Discovered** on a screen means a component that the walker has an action for (a link, button, tab, menu item,
//! checkbox, radio, switch, disclosure, text box, search box, combo box or list box) with at least one instance that
//! is visible in the screen's first reading. A component that is on the header or footer of every page counts on the
//! first screen that shows it and not again, which is how the walk treats it, so totals are sums with nothing counted
//! twice. Every discovered component of a screen is exactly one of:
//!
//! - **touched**: an action was made on it on this screen (an edge whose status is not `missing`);
//! - **skipped**, with the reason: left alone by the rules (`destructive`, `blocked`, `read-only`), by the walker
//!   (`no-options`), or because it could not be used (`disabled`);
//! - **untouched**: nothing was done, because the walk ended first, the screen could not be opened again, or the screen
//!   was deeper than `--max-depth`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::graph::{Graph, Status};

/// The version of the file's layout.
const COVERAGE_VERSION: u32 = 1;

/// A component found on a screen when it was first read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub component: String,
    /// `role "name"`, for people reading the file.
    pub label: String,
    /// Visible and enabled: something a person could act on.
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCoverage {
    pub state: String,
    pub url: String,
    pub discovered: Vec<String>,
    pub touched: Vec<String>,
    /// Component and reason, sorted by component.
    pub skipped: Vec<(String, String)>,
    pub untouched: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub start_url: String,
    pub stopped: &'static str,
    pub states: Vec<StateCoverage>,
    /// The ids on every page, counted on the first screen that shows them.
    pub shared: Vec<String>,
    pub unreachable: Vec<String>,
    /// Every skipped component with its label and reason, for people reading the file.
    pub skipped: Vec<SkippedItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedItem {
    pub state: String,
    pub component: String,
    pub label: String,
    pub reason: String,
}

/// The sums over every screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub discovered: usize,
    pub touched: usize,
    pub skipped: usize,
    pub untouched: usize,
}

impl Totals {
    /// `touched / discovered` in thousandths, rounded down. 0 when nothing was discovered.
    pub fn per_mille(self) -> usize {
        (self.touched * 1000).checked_div(self.discovered).unwrap_or(0)
    }

    /// What the walk was allowed to touch: everything discovered that was not left alone.
    pub fn allowed(self) -> usize {
        self.discovered - self.skipped
    }

    /// `touched / allowed` in thousandths, rounded down: how much of what the walk could do it did. A read-only walk
    /// may not press most buttons, so this is the fairer figure for it. 0 when nothing was allowed.
    pub fn allowed_per_mille(self) -> usize {
        (self.touched * 1000).checked_div(self.allowed()).unwrap_or(0)
    }
}

impl Coverage {
    /// `found` holds, for each screen of the graph in order, what its first reading showed.
    pub fn build(graph: &Graph, found: &[Vec<Discovered>], shared: &[String]) -> Self {
        // What was done on each screen, by component.
        let mut touched: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for edge in graph.edges.iter().filter(|edge| edge.status != Status::Missing) {
            touched.entry(&edge.from).or_default().insert(&edge.component);
        }
        let mut skipped_by_state: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
        for item in &graph.skipped {
            skipped_by_state
                .entry(&item.state)
                .or_default()
                .insert(&item.component, &item.reason);
        }

        let mut skipped_items: Vec<SkippedItem> = graph
            .skipped
            .iter()
            .map(|item| SkippedItem {
                state: item.state.clone(),
                component: item.component.clone(),
                label: item.label.clone(),
                reason: item.reason.clone(),
            })
            .collect();

        let mut states = Vec::new();
        for (index, state) in graph.states.iter().enumerate() {
            let on_screen = found.get(index).map(Vec::as_slice).unwrap_or_default();
            let done = touched.get(state.id.as_str());
            let left_alone = skipped_by_state.get(state.id.as_str());
            let mut coverage = StateCoverage {
                state: state.id.clone(),
                url: state.url.clone(),
                discovered: Vec::new(),
                touched: Vec::new(),
                skipped: Vec::new(),
                untouched: Vec::new(),
            };
            let mut seen = BTreeSet::new();
            let mut sorted: Vec<&Discovered> = on_screen.iter().collect();
            sorted.sort_by(|a, b| a.component.cmp(&b.component));
            for item in sorted {
                if !seen.insert(item.component.as_str()) {
                    continue;
                }
                let id = item.component.as_str();
                coverage.discovered.push(id.to_string());
                if done.is_some_and(|set| set.contains(id)) {
                    coverage.touched.push(id.to_string());
                } else if let Some(reason) = left_alone.and_then(|map| map.get(id)) {
                    coverage.skipped.push((id.to_string(), (*reason).to_string()));
                } else if !item.actionable {
                    coverage.skipped.push((id.to_string(), "disabled".to_string()));
                    skipped_items.push(SkippedItem {
                        state: state.id.clone(),
                        component: id.to_string(),
                        label: item.label.clone(),
                        reason: "disabled".to_string(),
                    });
                } else {
                    coverage.untouched.push(id.to_string());
                }
            }
            states.push(coverage);
        }
        skipped_items.sort_by(|a, b| (&a.state, &a.component).cmp(&(&b.state, &b.component)));
        // A component that was skipped and then touched on the same screen is only in `touched`.
        skipped_items.retain(|item| {
            !touched
                .get(item.state.as_str())
                .is_some_and(|set| set.contains(item.component.as_str()))
        });

        Self {
            start_url: graph.settings.start_url.clone(),
            stopped: graph.stopped.as_str(),
            states,
            shared: shared.to_vec(),
            unreachable: graph.unreachable.clone(),
            skipped: skipped_items,
        }
    }

    pub fn totals(&self) -> Totals {
        self.states.iter().fold(Totals::default(), |total, state| Totals {
            discovered: total.discovered + state.discovered.len(),
            touched: total.touched + state.touched.len(),
            skipped: total.skipped + state.skipped.len(),
            untouched: total.untouched + state.untouched.len(),
        })
    }

    pub fn to_json(&self) -> Value {
        let totals = self.totals();
        json!({
            "schemaVersion": COVERAGE_VERSION,
            "startUrl": self.start_url,
            "stopped": self.stopped,
            "totals": counts(totals, true),
            "states": self.states.iter().map(|state| json!({
                "id": state.state,
                "url": state.url,
                "counts": counts(
                    Totals {
                        discovered: state.discovered.len(),
                        touched: state.touched.len(),
                        skipped: state.skipped.len(),
                        untouched: state.untouched.len(),
                    },
                    false,
                ),
                "discovered": state.discovered,
                "touched": state.touched,
                "skipped": state.skipped.iter().map(|(component, reason)| json!({
                    "component": component,
                    "reason": reason,
                })).collect::<Vec<_>>(),
                "untouched": state.untouched,
            })).collect::<Vec<_>>(),
            "shared": self.shared,
            "unreachable": self.unreachable,
            "skipped": self.skipped.iter().map(|item| json!({
                "state": item.state,
                "component": item.component,
                "label": item.label,
                "reason": item.reason,
            })).collect::<Vec<_>>(),
        })
    }

    /// The bytes of `coverage.json`: pretty, keys sorted, a newline at the end.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        text.push('\n');
        text.into_bytes()
    }

    /// One line for the end of a run.
    pub fn summary(&self) -> String {
        let totals = self.totals();
        let share = |per_mille: usize| format!("{}.{}%", per_mille / 10, per_mille % 10);
        format!(
            "Coverage: {} of {} components touched ({}), {} left alone, {} not reached. Of the {} it was allowed to touch: {}",
            totals.touched,
            totals.discovered,
            share(totals.per_mille()),
            totals.skipped,
            totals.untouched,
            totals.allowed(),
            share(totals.allowed_per_mille())
        )
    }
}

fn counts(totals: Totals, with_share: bool) -> Value {
    let mut value = json!({
        "discovered": totals.discovered,
        "touched": totals.touched,
        "skipped": totals.skipped,
        "untouched": totals.untouched,
    });
    if with_share {
        value["touchedPerMille"] = json!(totals.per_mille());
        value["touchedOfAllowedPerMille"] = json!(totals.allowed_per_mille());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::super::graph::{Edge, Settings, Skipped, State, Stop};
    use super::*;

    fn settings() -> Settings {
        Settings {
            start_url: "http://shop.test/".to_string(),
            seed: 0,
            strategy: "bfs",
            similarity_per_mille: 900,
            max_states: 50,
            max_actions: 300,
            max_depth: 6,
            action_timeout_ms: 10_000,
            timeout_ms: 600_000,
            max_requests_per_origin: 2000,
        }
    }

    fn state(id: &str) -> State {
        State {
            id: id.to_string(),
            url: format!("http://shop.test/{id}"),
            modal: None,
            components: Vec::new(),
            frames: Vec::new(),
            depth: 0,
        }
    }

    fn edge(from: &str, component: &str, status: Status) -> Edge {
        Edge {
            id: "e0".to_string(),
            from: from.to_string(),
            component: component.to_string(),
            label: String::new(),
            action: "click",
            to: None,
            status,
            navigated: false,
            popups: 0,
            dialogs: 0,
            downloads: 0,
        }
    }

    fn found(id: &str, actionable: bool) -> Discovered {
        Discovered {
            component: id.to_string(),
            label: format!("button \"{id}\""),
            actionable,
        }
    }

    fn graph(states: usize, edges: Vec<Edge>, skipped: Vec<Skipped>) -> Graph {
        Graph {
            settings: settings(),
            chrome_pages: 0,
            chrome: Vec::new(),
            stopped: Stop::Complete,
            actions: edges.len() as u32,
            states: (0..states).map(|n| state(&format!("s{n}"))).collect(),
            edges,
            skipped,
            unreachable: Vec::new(),
        }
    }

    #[test]
    fn every_discovered_component_is_touched_skipped_or_untouched_and_the_totals_are_the_sums() {
        let graph = graph(
            2,
            vec![
                edge("s0", "c_link", Status::Ok),
                edge("s0", "c_slow", Status::Timeout),
                edge("s0", "c_gone", Status::Missing),
            ],
            vec![Skipped {
                state: "s0".to_string(),
                component: "c_delete".to_string(),
                label: "button \"Delete\"".to_string(),
                reason: "destructive".to_string(),
            }],
        );
        let coverage = Coverage::build(
            &graph,
            &[
                vec![
                    found("c_link", true),
                    found("c_slow", true),
                    found("c_gone", true),
                    found("c_delete", true),
                    found("c_off", false),
                ],
                vec![found("c_later", true)],
            ],
            &[],
        );
        let first = &coverage.states[0];
        assert_eq!(
            first.touched,
            ["c_link", "c_slow"],
            "a timeout is an action that was made"
        );
        assert_eq!(first.untouched, ["c_gone"], "a component that was gone was not touched");
        assert_eq!(
            first.skipped,
            [
                ("c_delete".to_string(), "destructive".to_string()),
                ("c_off".to_string(), "disabled".to_string())
            ]
        );
        assert_eq!(coverage.states[1].untouched, ["c_later"]);
        let totals = coverage.totals();
        assert_eq!(
            totals,
            Totals {
                discovered: 6,
                touched: 2,
                skipped: 2,
                untouched: 2
            }
        );
        assert_eq!(totals.per_mille(), 333);
        assert_eq!(totals.allowed(), 4, "what was not left alone");
        assert_eq!(totals.allowed_per_mille(), 500, "2 of the 4 it could touch");
        for state in &coverage.states {
            assert_eq!(
                state.discovered.len(),
                state.touched.len() + state.skipped.len() + state.untouched.len()
            );
        }
        assert!(coverage
            .skipped
            .iter()
            .any(|s| s.reason == "disabled" && s.component == "c_off"));
        assert!(coverage
            .summary()
            .starts_with("Coverage: 2 of 6 components touched (33.3%)"));
    }

    #[test]
    fn a_component_that_was_skipped_and_touched_on_one_screen_counts_once_as_touched() {
        let graph = graph(
            1,
            vec![edge("s0", "c_a", Status::Ok)],
            vec![Skipped {
                state: "s0".to_string(),
                component: "c_a".to_string(),
                label: String::new(),
                reason: "read-only".to_string(),
            }],
        );
        let coverage = Coverage::build(&graph, &[vec![found("c_a", true)]], &[]);
        assert_eq!(coverage.states[0].touched, ["c_a"]);
        assert!(coverage.states[0].skipped.is_empty());
        assert!(coverage.skipped.is_empty());
    }

    #[test]
    fn a_component_listed_twice_for_a_screen_counts_once_and_the_order_does_not_matter() {
        let graph = graph(1, vec![], vec![]);
        let one = Coverage::build(
            &graph,
            &[vec![found("c_b", true), found("c_a", true), found("c_b", true)]],
            &[],
        );
        let other = Coverage::build(&graph, &[vec![found("c_a", true), found("c_b", true)]], &[]);
        assert_eq!(one.states[0].discovered, ["c_a", "c_b"]);
        assert_eq!(one.to_bytes(), other.to_bytes());
    }

    #[test]
    fn nothing_discovered_is_zero_per_mille_and_not_a_division_by_zero() {
        let coverage = Coverage::build(&graph(1, vec![], vec![]), &[vec![]], &[]);
        assert_eq!(coverage.totals(), Totals::default());
        assert_eq!(coverage.totals().per_mille(), 0);
        assert_eq!(coverage.totals().allowed_per_mille(), 0);
        assert!(coverage.summary().contains("0 of 0"));
    }

    #[test]
    fn the_file_has_the_keys_the_schema_lists_and_ends_with_a_newline() {
        let coverage = Coverage::build(
            &graph(1, vec![], vec![]),
            &[vec![found("c_a", true)]],
            &["c_nav".to_string()],
        );
        let json = coverage.to_json();
        for key in [
            "schemaVersion",
            "startUrl",
            "stopped",
            "totals",
            "states",
            "shared",
            "unreachable",
            "skipped",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert_eq!(json["totals"]["touchedPerMille"], 0);
        assert_eq!(json["totals"]["touchedOfAllowedPerMille"], 0);
        assert!(json["states"][0]["counts"].get("touchedPerMille").is_none());
        assert!(coverage.to_bytes().ends_with(b"}\n"));
    }
}
