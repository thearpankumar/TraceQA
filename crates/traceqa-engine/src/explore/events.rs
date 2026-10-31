//! What the walk tells whoever is watching it, as it goes (backlog EXP-7).
//!
//! Each [`WalkEvent`] becomes one line of `events.jsonl` with a type, a sentence and a few fields, so that a tool can
//! follow a walk without parsing sentences. A person watching the terminal sees the sentences of most of them
//! (the command lists which: an action's start and what the rules left alone are in the file only).
//!
//! | type                    | when |
//! |-------------------------|------|
//! | `progress`              | a plain line that has no event of its own (a screen given up, a page that would not load) |
//! | `explore.state.new`     | a screen is found, the start page included |
//! | `explore.action.start`  | an action is about to be made; it has the id its edge will have |
//! | `explore.action.end`    | an edge is recorded, for every edge there is (also one whose component was gone, which had no start) |
//! | `explore.policy.block`  | a component was left alone by the rules, or the browser refused a request |
//! | `explore.replay`        | an attempt to reopen a screen by replaying actions, told also when it failed; its actions make no edge |
//! | `model.call`            | a model was asked for the text of a field (EXP-6) |
//!
//! `explore.action.end` is written once for each edge of `graph.json`. A start has no end when the walk ended in the
//! middle of that action: Ctrl-C or the wall clock, a browser that was lost, or `max-states` (the action that would
//! have found one screen too many is not kept). It can only be the last action of the walk.

use serde_json::{json, Value};

use super::graph::Edge;
use super::values::ModelCall;

/// One thing that happened in a walk. It borrows what it says, so making one costs nothing when nobody listens.
pub enum WalkEvent<'a> {
    /// A plain line.
    Line(&'a str),
    StateNew {
        state: &'a str,
        depth: u32,
        /// The cleaned address, with secrets hidden.
        url: &'a str,
    },
    ActionStart {
        /// The id the edge of this action will have.
        edge: &'a str,
        state: &'a str,
        component: &'a str,
        label: &'a str,
        action: &'a str,
    },
    ActionEnd {
        edge: &'a Edge,
        /// Why it did not work, when it did not. Not in the file.
        note: Option<&'a str>,
    },
    /// The rules left a component alone.
    ComponentSkipped {
        state: &'a str,
        component: &'a str,
        label: &'a str,
        reason: &'a str,
    },
    /// The browser refused a request that the page made because of an action.
    RequestRefused {
        edge: &'a str,
        method: &'a str,
        url: &'a str,
    },
    /// One attempt to reopen a screen by replaying the actions that led to it. Told also when it failed: the actions
    /// it made count against the budget and make no edge.
    Replay {
        state: &'a str,
        /// How many actions the attempt made.
        actions: usize,
        reopened: bool,
    },
    ModelCall(&'a ModelCall),
}

impl WalkEvent<'_> {
    /// The `type` of the event in `events.jsonl`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Line(_) => "progress",
            Self::StateNew { .. } => "explore.state.new",
            Self::ActionStart { .. } => "explore.action.start",
            Self::ActionEnd { .. } => "explore.action.end",
            Self::ComponentSkipped { .. } | Self::RequestRefused { .. } => "explore.policy.block",
            Self::Replay { .. } => "explore.replay",
            Self::ModelCall(_) => "model.call",
        }
    }

    /// One sentence for a person.
    pub fn message(&self) -> String {
        match self {
            Self::Line(line) => (*line).to_string(),
            Self::StateNew { state, depth, url } => format!("{state} is new (depth {depth}): {url}"),
            Self::ActionStart {
                edge,
                state,
                label,
                action,
                ..
            } => format!("{edge}: {action} {label} on {state}"),
            Self::ActionEnd { edge, note } => {
                let end = match (&edge.to, note) {
                    (Some(to), _) => to.clone(),
                    (None, Some(note)) => format!("{} ({note})", edge.status.as_str()),
                    (None, None) => edge.status.as_str().to_string(),
                };
                format!("{}: {} {} on {} -> {end}", edge.id, edge.action, edge.label, edge.from)
            }
            Self::ComponentSkipped {
                state, label, reason, ..
            } => format!("{state}: left {label} alone ({reason})"),
            Self::RequestRefused { edge, method, url } => {
                format!("{edge}: the browser refused a {method} request to {url}")
            }
            Self::Replay {
                state,
                actions,
                reopened: true,
            } => format!("{state} reopened by replaying {actions} actions"),
            Self::Replay {
                state,
                actions,
                reopened: false,
            } => format!("{state} was not brought back by replaying ({actions} actions made)"),
            Self::ModelCall(call) => call.summary(),
        }
    }

    /// The `data` of the event.
    pub fn data(&self) -> Value {
        match self {
            Self::Line(_) => json!({}),
            Self::StateNew { state, depth, url } => json!({ "state": state, "depth": depth, "url": url }),
            Self::ActionStart {
                edge,
                state,
                component,
                label,
                action,
            } => json!({ "edge": edge, "state": state, "component": component, "label": label, "action": action }),
            Self::ActionEnd { edge, .. } => {
                let mut data = json!({
                    "edge": edge.id,
                    "state": edge.from,
                    "component": edge.component,
                    "label": edge.label,
                    "action": edge.action,
                    "status": edge.status.as_str(),
                    "navigated": edge.navigated,
                    "popups": edge.popups,
                    "dialogs": edge.dialogs,
                    "downloads": edge.downloads,
                });
                if let Some(to) = &edge.to {
                    data["to"] = json!(to);
                }
                data
            }
            Self::ComponentSkipped {
                state,
                component,
                label,
                reason,
            } => {
                json!({ "kind": "component", "state": state, "component": component, "label": label, "reason": reason })
            }
            Self::RequestRefused { edge, method, url } => {
                json!({ "kind": "request", "edge": edge, "method": method, "url": url, "reason": "blocked_by_policy" })
            }
            Self::Replay {
                state,
                actions,
                reopened,
            } => json!({ "state": state, "actions": actions, "reopened": reopened }),
            Self::ModelCall(call) => call.data(),
        }
    }
}
