//! The result of a walk: screens, the actions between them, what was left alone, and why the walk ended.
//!
//! `graph.json` is built from a [`serde_json::Value`], whose objects keep their keys sorted, and every list is in the
//! order things happened, so two walks that made the same moves write the same bytes. Nothing in it is a time.

use serde::Serialize;
use serde_json::{json, Value};

/// The version of the `graph.json` layout (`spec/graph.schema.json`).
pub const GRAPH_VERSION: u32 = 1;

/// Why a walk ended. The words are the names of the budgets (`--max-actions` is `max-actions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Stop {
    /// Every action that could be tried was tried.
    #[serde(rename = "complete")]
    Complete,
    #[serde(rename = "max-states")]
    MaxStates,
    #[serde(rename = "max-actions")]
    MaxActions,
    /// Some screens were not explored because they are deeper than the limit, and nothing else was left.
    #[serde(rename = "max-depth")]
    MaxDepth,
    /// Several actions in a row used up all the time they were given.
    #[serde(rename = "action-timeout")]
    ActionTimeout,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "max-requests")]
    MaxRequests,
    #[serde(rename = "cancelled")]
    Cancelled,
    /// The browser failed. The message is in the run's events, not in this file.
    #[serde(rename = "error")]
    Error,
}

impl Stop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::MaxStates => "max-states",
            Self::MaxActions => "max-actions",
            Self::MaxDepth => "max-depth",
            Self::ActionTimeout => "action-timeout",
            Self::Timeout => "timeout",
            Self::MaxRequests => "max-requests",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

/// How an action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// The element was no longer on the page.
    Missing,
    Timeout,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub id: String,
    /// The cleaned address, with secret parameters hidden.
    pub url: String,
    pub modal: Option<String>,
    pub components: Vec<String>,
    pub frames: Vec<String>,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub component: String,
    /// `role "name"`, for people reading the file.
    pub label: String,
    pub action: &'static str,
    /// The screen the action led to. `None` when it did not complete.
    pub to: Option<String>,
    pub status: Status,
    pub navigated: bool,
    pub popups: u32,
    pub dialogs: u32,
    pub downloads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub state: String,
    pub component: String,
    pub label: String,
    /// `destructive`, `blocked`, `read-only`, or one of the walker's own (`no-options`, `gone`).
    pub reason: String,
}

/// What the walk was told, as it is written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub start_url: String,
    pub seed: u64,
    pub strategy: &'static str,
    pub similarity_per_mille: u16,
    pub max_states: u32,
    pub max_actions: u32,
    pub max_depth: u32,
    pub action_timeout_ms: u64,
    pub timeout_ms: u64,
    pub max_requests_per_origin: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graph {
    pub settings: Settings,
    /// How many pages were read to find out what is the same on every page.
    pub chrome_pages: u32,
    /// The ids that are on (nearly) every page. They are left out of the ids that screens are compared by.
    pub chrome: Vec<String>,
    pub stopped: Stop,
    pub actions: u32,
    pub states: Vec<State>,
    pub edges: Vec<Edge>,
    pub skipped: Vec<Skipped>,
    /// Screens that could not be opened again, so their remaining actions were not tried.
    pub unreachable: Vec<String>,
}

impl Graph {
    pub fn to_json(&self) -> Value {
        let s = &self.settings;
        json!({
            "schemaVersion": GRAPH_VERSION,
            "startUrl": s.start_url,
            "seed": s.seed,
            "strategy": s.strategy,
            "stateSimilarityPerMille": s.similarity_per_mille,
            "chromePages": self.chrome_pages,
            "chrome": self.chrome,
            "budgets": {
                "maxStates": s.max_states,
                "maxActions": s.max_actions,
                "maxDepth": s.max_depth,
                "actionTimeoutMs": s.action_timeout_ms,
                "timeoutMs": s.timeout_ms,
                "maxRequestsPerOrigin": s.max_requests_per_origin,
            },
            "stopped": self.stopped.as_str(),
            "actions": self.actions,
            "states": self.states.iter().map(|state| {
                let mut value = json!({
                    "id": state.id,
                    "url": state.url,
                    "components": state.components,
                    "frames": state.frames,
                    "depth": state.depth,
                });
                if let Some(modal) = &state.modal {
                    value["modal"] = json!(modal);
                }
                value
            }).collect::<Vec<_>>(),
            "edges": self.edges.iter().map(|edge| {
                let mut value = json!({
                    "id": edge.id,
                    "from": edge.from,
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
                    value["to"] = json!(to);
                }
                value
            }).collect::<Vec<_>>(),
            "skipped": self.skipped.iter().map(|item| json!({
                "state": item.state,
                "component": item.component,
                "label": item.label,
                "reason": item.reason,
            })).collect::<Vec<_>>(),
            "unreachable": self.unreachable,
        })
    }

    /// The bytes of `graph.json`: pretty, keys sorted, a newline at the end.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        text.push('\n');
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_of_a_status_and_a_stop_are_the_ones_the_schema_lists() {
        assert_eq!(
            [Status::Ok, Status::Missing, Status::Timeout, Status::Failed].map(Status::as_str),
            ["ok", "missing", "timeout", "failed"]
        );
        assert_eq!(
            [
                Stop::Complete,
                Stop::MaxStates,
                Stop::MaxActions,
                Stop::MaxDepth,
                Stop::ActionTimeout,
                Stop::Timeout,
                Stop::MaxRequests,
                Stop::Cancelled,
                Stop::Error
            ]
            .map(Stop::as_str),
            [
                "complete",
                "max-states",
                "max-actions",
                "max-depth",
                "action-timeout",
                "timeout",
                "max-requests",
                "cancelled",
                "error"
            ]
        );
    }
}
