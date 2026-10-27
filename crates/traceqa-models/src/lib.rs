//! Sources of text for the fields `traceqa explore` types into (backlog EXP-6).
//!
//! The engine knows the [`ValueSource`] trait and nothing else about models. What implements it lives here, so that
//! the engine never depends on an HTTP client or a model library (a test in the engine checks `cargo tree`).
//!
//! Today there is one source: `mock`, which sends nothing anywhere. The connections to real models (a compatible
//! HTTP endpoint, Amazon Bedrock) are the next step and are added here; see the notes on EXP-6 in `docs/backlog.md`.
//! A source that sends text out of this machine must also need an extra flag on the command, like exploring a site
//! that is not local does.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use traceqa_engine::explore::{Field, SourceError, ValueSource};

/// The names `--model` takes.
pub const KNOWN: &str = "mock";

/// Makes the source `--model` names. `mock` takes options after a colon, separated by commas:
///
/// - `sleep=MS`: answer after MS milliseconds (a slow model);
/// - `fail`: always give an error;
/// - `junk`: answer with text that has a line break, so it is thrown away.
///
/// `mock:sleep=60000` is what the Ctrl-C test uses.
pub fn source_from_spec(spec: &str) -> Result<Arc<dyn ValueSource>, String> {
    let (name, options) = spec.split_once(':').unwrap_or((spec, ""));
    match name {
        "mock" => Ok(Arc::new(Mock::from_options(options)?)),
        other => Err(format!(
            "no model is called \"{other}\". The one that exists is: {KNOWN}. Connections to real models are not built yet."
        )),
    }
}

/// A stand-in for a model. It answers `mock <label>` (an address for an email field, a number for a number field),
/// so that what it typed can be told from what the generator types. It sends nothing anywhere.
#[derive(Debug, Default)]
pub struct Mock {
    sleep: Duration,
    fail: bool,
    junk: bool,
}

impl Mock {
    fn from_options(options: &str) -> Result<Self, String> {
        let mut mock = Self::default();
        for option in options.split(',').filter(|o| !o.is_empty()) {
            match option.split_once('=') {
                Some(("sleep", ms)) => {
                    let ms: u64 = ms
                        .parse()
                        .map_err(|_| format!("mock: sleep={ms} is not a number of milliseconds"))?;
                    mock.sleep = Duration::from_millis(ms);
                }
                None if option == "fail" => mock.fail = true,
                None if option == "junk" => mock.junk = true,
                _ => {
                    return Err(format!(
                        "mock: unknown option \"{option}\" (known: sleep=MS, fail, junk)"
                    ))
                }
            }
        }
        Ok(mock)
    }
}

impl ValueSource for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn value(&self, field: &Field) -> Result<String, SourceError> {
        thread::sleep(self.sleep);
        if self.fail {
            return Err(SourceError("the mock model was told to fail".to_string()));
        }
        if self.junk {
            return Ok("first line\nsecond line".to_string());
        }
        let answer = match field.input_type.as_deref().map(str::to_lowercase).as_deref() {
            Some("email") => "mock@example.test".to_string(),
            Some("number" | "range") => "42".to_string(),
            Some("tel") => "5550142".to_string(),
            Some("url") => "https://example.test/mock".to_string(),
            _ => format!("mock {}", field.label.to_lowercase()),
        };
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(label: &str, kind: Option<&str>) -> Field {
        Field {
            role: "textbox".to_string(),
            label: label.to_string(),
            input_type: kind.map(str::to_string),
            ..Field::default()
        }
    }

    #[test]
    fn the_mock_answers_by_the_kind_of_field() {
        let mock = source_from_spec("mock").unwrap();
        assert_eq!(mock.value(&field("Search shop", None)).unwrap(), "mock search shop");
        assert_eq!(mock.value(&field("Mail", Some("email"))).unwrap(), "mock@example.test");
        assert_eq!(mock.value(&field("How many", Some("number"))).unwrap(), "42");
        assert_eq!(mock.name(), "mock");
    }

    #[test]
    fn the_mock_can_fail_give_junk_or_be_slow() {
        assert!(source_from_spec("mock:fail").unwrap().value(&field("x", None)).is_err());
        let junk = source_from_spec("mock:junk").unwrap().value(&field("x", None)).unwrap();
        assert!(junk.contains('\n'));
        let started = std::time::Instant::now();
        source_from_spec("mock:sleep=120,fail,junk")
            .unwrap()
            .value(&field("x", None))
            .ok();
        assert!(started.elapsed() >= Duration::from_millis(120));
    }

    #[test]
    fn a_name_that_is_not_known_or_a_bad_option_is_an_error_that_says_what_exists() {
        let error = source_from_spec("gpt").err().unwrap();
        assert!(error.contains("\"gpt\"") && error.contains("mock"), "{error}");
        assert!(source_from_spec("mock:sleep=soon")
            .err()
            .unwrap()
            .contains("sleep=soon"));
        assert!(source_from_spec("mock:loud").err().unwrap().contains("loud"));
    }
}
