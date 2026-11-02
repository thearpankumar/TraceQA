//! What a replay writes and tells: `replay.json` (`spec/replay.schema.json`) and the events of `events.jsonl`.
//!
//! Like `graph.json`, the file is built from a [`serde_json::Value`], whose objects keep their keys sorted, and its lists
//! are in the order the edges were asked for, so two replays that went the same way write the same bytes. Nothing in
//! it is a time.

use serde_json::{json, Value};

use super::replay::{Mode, Outcome, Replayed, Step};
use super::replay_doc::{Doc, DocEdge};

/// The version of the `replay.json` layout.
pub const REPLAY_VERSION: u32 = 1;

/// What the person or the tool watching is told, as it happens.
#[derive(Debug)]
pub enum ReplayEvent<'a> {
    /// A plain line.
    Line(&'a str),
    /// One edge dealt with.
    Step { edge: &'a DocEdge, step: &'a Step },
    /// One attempt to reopen a screen that has no address of its own, by performing the way to it. Told also when it
    /// failed: its actions count and make no step.
    Reopen {
        state: &'a str,
        actions: usize,
        reopened: bool,
    },
}

impl ReplayEvent<'_> {
    /// The `type` of the event in `events.jsonl`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Line(_) => "progress",
            Self::Step { .. } => "replay.step",
            Self::Reopen { .. } => "replay.reopen",
        }
    }

    /// One sentence for a person.
    pub fn message(&self) -> String {
        match self {
            Self::Line(line) => (*line).to_string(),
            Self::Step { edge, step } => {
                let mut line = format!(
                    "{}: {} {} on s{} -> {}",
                    edge.id,
                    edge.action.as_str(),
                    edge.label,
                    edge.from,
                    step.outcome.as_str()
                );
                if let Some(note) = &step.note {
                    line.push_str(&format!(" ({note})"));
                }
                line
            }
            Self::Reopen {
                state,
                actions,
                reopened: true,
            } => format!("{state} reopened by performing {actions} actions"),
            Self::Reopen {
                state,
                actions,
                reopened: false,
            } => format!("{state} was not brought back ({actions} actions made)"),
        }
    }

    /// The `data` of the event.
    pub fn data(&self) -> Value {
        match self {
            Self::Line(_) => json!({}),
            Self::Step { edge, step } => step_json(edge, step),
            Self::Reopen {
                state,
                actions,
                reopened,
            } => json!({ "state": state, "actions": actions, "reopened": reopened }),
        }
    }
}

fn step_json(edge: &DocEdge, step: &Step) -> Value {
    let mut value = json!({
        "edge": edge.id,
        "state": format!("s{}", edge.from),
        "component": edge.component,
        "label": edge.label,
        "action": edge.action.as_str(),
        "outcome": step.outcome.as_str(),
    });
    if let Some(to) = edge.to {
        value["expected"] = json!(format!("s{to}"));
    }
    if let Some(actual) = &step.actual {
        value["actual"] = json!(actual);
    }
    if let Some(note) = &step.note {
        value["note"] = json!(note);
    }
    value
}

/// How the steps came out. `replayable` is the steps that were asked for and that the walk had completed: the edges
/// it had not are `recorded-failed` and are not counted against the replay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub replayable: usize,
    pub reproduced: usize,
    pub diverged: usize,
    pub missing: usize,
    pub blocked: usize,
    pub not_run: usize,
    pub recorded_failed: usize,
}

impl Totals {
    pub fn of(steps: &[Step]) -> Self {
        let mut totals = Self::default();
        for step in steps {
            match step.outcome {
                Outcome::Reproduced => totals.reproduced += 1,
                Outcome::Diverged => totals.diverged += 1,
                Outcome::Missing => totals.missing += 1,
                Outcome::Blocked => totals.blocked += 1,
                Outcome::NotRun => totals.not_run += 1,
                Outcome::RecordedFailed => totals.recorded_failed += 1,
            }
        }
        totals.replayable = steps.len() - totals.recorded_failed;
        totals
    }

    /// The share of the replayable steps that were reproduced, in thousandths. `None` when none were asked for.
    pub fn per_mille(&self) -> Option<u64> {
        (self.replayable > 0).then(|| (self.reproduced as u64 * 1000) / self.replayable as u64)
    }

    /// Every step that could be replayed was reproduced.
    pub fn all_reproduced(&self) -> bool {
        self.reproduced == self.replayable
    }
}

impl Replayed {
    pub fn totals(&self) -> Totals {
        Totals::of(&self.steps)
    }

    /// Whether the replay is a success: it ended on its own and every replayable step was reproduced.
    pub fn succeeded(&self) -> bool {
        self.ended == super::replay::Ended::Complete && self.totals().all_reproduced()
    }

    pub fn to_json(&self, doc: &Doc, mode: &Mode) -> Value {
        let totals = self.totals();
        let mut counts = json!({
            "replayable": totals.replayable,
            "reproduced": totals.reproduced,
            "diverged": totals.diverged,
            "missing": totals.missing,
            "blocked": totals.blocked,
            "notRun": totals.not_run,
            "recordedFailed": totals.recorded_failed,
        });
        if let Some(per_mille) = totals.per_mille() {
            counts["reproducedPerMille"] = json!(per_mille);
        }
        json!({
            "schemaVersion": REPLAY_VERSION,
            "graph": {
                "schemaVersion": doc.version,
                "startUrl": crate::url_safety::redact(&doc.start_url),
                "states": doc.states.len(),
                "edges": doc.edges.len(),
            },
            "mode": mode.as_str(),
            "ended": self.ended.as_str(),
            "actions": self.actions,
            "totals": counts,
            "steps": self.steps.iter().map(|step| step_json(&doc.edges[step.edge], step)).collect::<Vec<_>>(),
        })
    }

    /// The bytes of `replay.json`: pretty, keys sorted, a newline at the end.
    pub fn to_bytes(&self, doc: &Doc, mode: &Mode) -> Vec<u8> {
        let mut text = serde_json::to_string_pretty(&self.to_json(doc, mode)).unwrap_or_default();
        text.push('\n');
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explore::replay::Ended;

    fn step(outcome: Outcome) -> Step {
        Step {
            edge: 0,
            outcome,
            actual: None,
            note: None,
        }
    }

    #[test]
    fn the_totals_count_each_outcome_and_leave_recorded_failures_out_of_the_replayable() {
        let steps = [
            Outcome::Reproduced,
            Outcome::Reproduced,
            Outcome::Diverged,
            Outcome::Missing,
            Outcome::Blocked,
            Outcome::NotRun,
            Outcome::RecordedFailed,
        ]
        .map(step);
        let totals = Totals::of(&steps);
        assert_eq!(totals.replayable, 6);
        assert_eq!((totals.reproduced, totals.diverged, totals.missing), (2, 1, 1));
        assert_eq!((totals.blocked, totals.not_run, totals.recorded_failed), (1, 1, 1));
        assert_eq!(totals.per_mille(), Some(333));
        assert!(!totals.all_reproduced());
    }

    #[test]
    fn a_replay_of_nothing_has_no_share_and_counts_as_all_reproduced() {
        let totals = Totals::of(&[step(Outcome::RecordedFailed)]);
        assert_eq!(totals.per_mille(), None);
        assert!(totals.all_reproduced());
    }

    #[test]
    fn only_a_replay_that_ended_on_its_own_with_every_step_reproduced_succeeded() {
        let replayed = |ended, outcomes: &[Outcome]| Replayed {
            steps: outcomes.iter().copied().map(step).collect(),
            ended,
            actions: 0,
            error: None,
        };
        assert!(replayed(Ended::Complete, &[Outcome::Reproduced, Outcome::RecordedFailed]).succeeded());
        assert!(!replayed(Ended::Complete, &[Outcome::Reproduced, Outcome::Diverged]).succeeded());
        assert!(!replayed(Ended::Timeout, &[Outcome::Reproduced]).succeeded());
        assert!(!replayed(Ended::Cancelled, &[Outcome::Reproduced]).succeeded());
    }

    #[test]
    fn the_words_of_an_outcome_and_an_ending_are_the_ones_the_schema_lists() {
        assert_eq!(
            [
                Outcome::Reproduced,
                Outcome::Diverged,
                Outcome::Missing,
                Outcome::Blocked,
                Outcome::NotRun,
                Outcome::RecordedFailed
            ]
            .map(Outcome::as_str),
            [
                "reproduced",
                "diverged",
                "missing",
                "blocked",
                "not-run",
                "recorded-failed"
            ]
        );
        assert_eq!(
            [
                Ended::Complete,
                Ended::Timeout,
                Ended::MaxRequests,
                Ended::Cancelled,
                Ended::Error
            ]
            .map(Ended::as_str),
            ["complete", "timeout", "max-requests", "cancelled", "error"]
        );
    }
    #[test]
    fn the_bytes_are_the_pretty_json_with_a_newline_at_the_end() {
        let replayed = Replayed {
            steps: vec![],
            ended: Ended::Complete,
            actions: 0,
            error: None,
        };
        let doc = Doc::parse(
            &json!({
                "schemaVersion": 2, "startUrl": "http://127.0.0.1:1/", "stateSimilarityPerMille": 900, "chrome": [],
                "states": [{ "id": "s0", "url": "http://127.0.0.1:1/", "components": ["c_a"], "frames": [], "depth": 0 }],
                "edges": [],
            })
            .to_string(),
        )
        .unwrap();
        let bytes = replayed.to_bytes(&doc, &Mode::All);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let back: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, replayed.to_json(&doc, &Mode::All));
        assert_eq!(back["schemaVersion"], REPLAY_VERSION);
    }
}
