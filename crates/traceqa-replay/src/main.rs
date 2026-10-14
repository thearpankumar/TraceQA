//! A stand-in for `traceqa serve` that plays back one transcript from `spec/transcripts/`.
//!
//!     TRACEQA_REPLAY=spec/transcripts/extract.jsonl \
//!     TRACEQA_REPLAY_REPORT=report.txt \
//!     traceqa-replay serve
//!
//! A client under test starts this program instead of the engine. Every line the client
//! writes must match the transcript's next `client` line, and the `engine` lines are written
//! back in between. When the conversation ends, the report file says `ok` or what went wrong.
//! The matching rules are in `spec/transcripts/README.md`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use serde_json::{json, Value};

/// A field whose expected value is this may hold anything, or be left out.
const ANY: &str = "<any>";

#[derive(Debug)]
enum Step {
    Client(Value),
    Engine(Value),
}

fn load(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", index + 1))?;
        if let Some(message) = entry.get("client") {
            steps.push(Step::Client(message.clone()));
        } else if let Some(message) = entry.get("engine") {
            steps.push(Step::Engine(message.clone()));
        } else if entry.get("note").is_none() {
            return Err(format!("line {}: expected a client, engine or note entry", index + 1));
        }
    }
    Ok(steps)
}

/// Why `actual` does not match `expected`, or None when it does. `at` names the place for the message.
fn difference(expected: &Value, actual: &Value, at: &str) -> Option<String> {
    let place = |key: &dyn std::fmt::Display| {
        if at.is_empty() {
            key.to_string()
        } else {
            format!("{at}.{key}")
        }
    };
    match (expected, actual) {
        (Value::String(any), _) if any == ANY => None,
        (Value::Number(x), Value::Number(y)) if x.as_f64() == y.as_f64() => None,
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(index, (x, y))| difference(x, y, &format!("{at}[{index}]"))),
        (Value::Object(x), Value::Object(y)) => {
            for (key, expected) in x {
                let found = match y.get(key) {
                    Some(actual) => difference(expected, actual, &place(key)),
                    None if expected == ANY => None,
                    None => Some(format!("{}: missing", place(key))),
                };
                if found.is_some() {
                    return found;
                }
            }
            y.keys()
                .find(|key| !x.contains_key(*key))
                .map(|key| format!("{}: not in the transcript", place(key)))
        }
        _ if expected == actual => None,
        _ => Some(format!(
            "{}: expected {expected}, got {actual}",
            if at.is_empty() { "message" } else { at }
        )),
    }
}

/// Plays the transcript against one client.
struct Replay {
    steps: Vec<Step>,
    next: usize,
    /// The transcript's request ids, mapped to the ids the client actually used.
    ids: HashMap<String, Value>,
}

impl Replay {
    /// Writes the engine lines that come before the next client line.
    fn speak(&mut self, output: &mut impl Write) -> io::Result<()> {
        while let Some(Step::Engine(message)) = self.steps.get(self.next) {
            let mut message = message.clone();
            if let Some(id) = message.get("id").map(Value::to_string) {
                if let Some(actual) = self.ids.get(&id) {
                    message["id"] = actual.clone();
                }
            }
            writeln!(output, "{message}")?;
            output.flush()?;
            self.next += 1;
        }
        Ok(())
    }

    /// Checks one line from the client against the transcript.
    fn hear(&mut self, line: &str) -> Result<(), String> {
        let actual: Value =
            serde_json::from_str(line).map_err(|e| format!("the client sent a line that is not JSON: {e}"))?;
        let Some(Step::Client(expected)) = self.steps.get(self.next) else {
            return Err(format!(
                "the client sent {actual}, but the transcript expected nothing more from it"
            ));
        };
        let mut without_id = actual.clone();
        let mut expected_without_id = expected.clone();
        let actual_id = without_id.as_object_mut().and_then(|message| message.remove("id"));
        let expected_id = expected_without_id
            .as_object_mut()
            .and_then(|message| message.remove("id"));
        if let Some(why) = difference(&expected_without_id, &without_id, "") {
            return Err(format!("{why}\n  expected {expected}\n  got      {actual}"));
        }
        match (expected_id, actual_id) {
            (Some(expected), Some(actual)) if actual.is_number() || actual.is_string() => {
                self.ids.insert(expected.to_string(), actual);
            }
            (Some(_), _) => return Err(format!("a request needs a number or string id, got {actual}")),
            (None, Some(_)) => return Err(format!("expected a notification, got a request: {actual}")),
            (None, None) => {}
        }
        self.next += 1;
        Ok(())
    }

    fn finished(&self) -> Result<(), String> {
        match self.steps.get(self.next) {
            None => Ok(()),
            Some(Step::Client(expected)) => Err(format!("the client stopped, but the transcript expected {expected}")),
            Some(Step::Engine(_)) => unreachable!("engine lines are written before reading"),
        }
    }
}

/// Runs the whole conversation. A mismatch also goes back to the client as an error answer,
/// so its test fails with the reason instead of only seeing the engine stop.
fn converse(steps: Vec<Step>, input: impl BufRead, output: &mut impl Write) -> Result<(), String> {
    let mut replay = Replay {
        steps,
        next: 0,
        ids: HashMap::new(),
    };
    let io_failure = |e: io::Error| format!("could not talk to the client: {e}");
    replay.speak(output).map_err(io_failure)?;
    for line in input.lines() {
        let line = line.map_err(io_failure)?;
        if line.trim().is_empty() {
            continue;
        }
        if let Err(why) = replay.hear(&line) {
            let id = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|message| message.get("id").cloned());
            if let Some(id) = id {
                let refusal = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": format!("transcript mismatch: {why}"),
                        "data": { "kind": "transcript_mismatch", "retryable": false }
                    }
                });
                let _ = writeln!(output, "{refusal}").and_then(|()| output.flush());
            }
            return Err(why);
        }
        replay.speak(output).map_err(io_failure)?;
    }
    replay.finished()
}

fn main() -> ExitCode {
    let Ok(path) = std::env::var("TRACEQA_REPLAY") else {
        eprintln!("traceqa-replay: set TRACEQA_REPLAY to a transcript file");
        return ExitCode::from(2);
    };
    let report = std::env::var_os("TRACEQA_REPLAY_REPORT");
    let outcome = fs::read_to_string(&path)
        .map_err(|e| format!("could not read {path}: {e}"))
        .and_then(|text| load(&text).map_err(|e| format!("{path}: {e}")))
        .and_then(|steps| converse(steps, io::stdin().lock(), &mut io::stdout().lock()));
    let text = match &outcome {
        Ok(()) => "ok".to_string(),
        Err(why) => why.clone(),
    };
    if let Some(report) = report {
        if let Err(e) = fs::write(&report, &text) {
            eprintln!("traceqa-replay: could not write the report: {e}");
        }
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("traceqa-replay: {why}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(transcript: &str, client: &str) -> (Result<(), String>, String) {
        let mut output = Vec::new();
        let outcome = converse(load(transcript).unwrap(), client.as_bytes(), &mut output);
        (outcome, String::from_utf8(output).unwrap())
    }

    const HELLO: &str = r#"{"note": "hello"}
{"client": {"jsonrpc": "2.0", "id": 1, "method": "engine.hello", "params": {"protocolVersion": 1, "clientName": "<any>"}}}
{"engine": {"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}}
{"engine": {"jsonrpc": "2.0", "method": "run.event", "params": {"n": 1}}}
"#;

    #[test]
    fn answers_with_the_clients_own_id() {
        let (outcome, output) = play(
            HELLO,
            r#"{"jsonrpc":"2.0","id":"a7","method":"engine.hello","params":{"protocolVersion":1.0,"clientName":"x"}}"#,
        );
        assert_eq!(outcome, Ok(()));
        let lines: Vec<Value> = output.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(lines[0]["id"], "a7");
        assert_eq!(lines[1]["method"], "run.event");
    }

    #[test]
    fn any_may_be_left_out() {
        let (outcome, _) = play(
            HELLO,
            r#"{"jsonrpc":"2.0","id":1,"method":"engine.hello","params":{"protocolVersion":1}}"#,
        );
        assert_eq!(outcome, Ok(()));
    }

    #[test]
    fn a_wrong_or_extra_field_is_refused_with_its_place() {
        let (outcome, output) = play(
            HELLO,
            r#"{"jsonrpc":"2.0","id":4,"method":"engine.hello","params":{"protocolVersion":2}}"#,
        );
        assert!(outcome
            .unwrap_err()
            .starts_with("params.protocolVersion: expected 1, got 2"));
        let refusal: Value = serde_json::from_str(output.lines().next().unwrap()).unwrap();
        assert_eq!(refusal["id"], 4);
        assert_eq!(refusal["error"]["data"]["kind"], "transcript_mismatch");

        let (outcome, _) = play(
            HELLO,
            r#"{"jsonrpc":"2.0","id":1,"method":"engine.hello","params":{"protocolVersion":1,"extra":null}}"#,
        );
        assert!(outcome.unwrap_err().starts_with("params.extra: not in the transcript"));
    }

    #[test]
    fn stopping_early_and_saying_too_much_are_both_failures() {
        let (outcome, _) = play(HELLO, "");
        assert!(outcome.unwrap_err().starts_with("the client stopped"));

        let hello = r#"{"jsonrpc":"2.0","id":1,"method":"engine.hello","params":{"protocolVersion":1}}"#;
        let (outcome, _) = play(HELLO, &format!("{hello}\n{hello}\n"));
        assert!(outcome.unwrap_err().contains("expected nothing more"));
    }

    #[test]
    fn every_transcript_in_spec_loads() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/transcripts");
        let mut loaded = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "jsonl") {
                let steps = load(&fs::read_to_string(&path).unwrap()).unwrap();
                assert!(
                    steps.iter().any(|step| matches!(step, Step::Client(_))),
                    "{}",
                    path.display()
                );
                loaded += 1;
            }
        }
        assert!(loaded > 0);
    }
}
