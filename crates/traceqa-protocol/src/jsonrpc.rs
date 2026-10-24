//! JSON-RPC 2.0 messages, one per line.
//!
//! This module only turns messages into lines and back. Reading and writing
//! the lines is the caller's job, so it works over any transport.

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::sync::LockExt;

const VERSION: &str = "2.0";

/// Request id. Either side may use numbers or strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(u64),
    String(String),
    /// Only in a response to a request whose id could not be read.
    Null,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// An `RpcErrorData` from the spec, when the sender provided one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard JSON-RPC error codes.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    /// The request cannot be a valid request, here because it is too large to read.
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// Any failure reported by a driver or the engine. Details are in `data`.
    pub const APPLICATION_ERROR: i64 = -32000;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request { id: Id, method: String, params: Value },
    Notification { method: String, params: Value },
    Response { id: Id, result: Result<Value, RpcError> },
}

/// Why a line is not a message.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DecodeError {
    #[error("not valid JSON: {0}")]
    Json(serde_json::Error),
    /// Valid JSON that is not a JSON-RPC 2.0 message.
    #[error("not a JSON-RPC 2.0 message: {0}")]
    Invalid(&'static str),
}

/// A message as it is read from a line.
#[derive(Deserialize)]
struct Incoming {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Id>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

/// A message as it is written to a line. It borrows, so a large `params` is not copied.
#[derive(Serialize)]
struct Outgoing<'a> {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a RpcError>,
}

/// Encodes a message as one line, without the trailing newline.
pub fn encode(message: &Message) -> String {
    let mut wire = Outgoing {
        jsonrpc: VERSION,
        id: None,
        method: None,
        params: None,
        result: None,
        error: None,
    };
    match message {
        Message::Request { id, method, params } => {
            wire.id = Some(id);
            wire.method = Some(method);
            wire.params = Some(params);
        }
        Message::Notification { method, params } => {
            wire.method = Some(method);
            wire.params = Some(params);
        }
        Message::Response { id, result } => {
            wire.id = Some(id);
            match result {
                // `result` must be present on success, even when it is null.
                Ok(value) => wire.result = Some(value),
                Err(error) => wire.error = Some(error),
            }
        }
    }
    serde_json::to_string(&wire).expect("a JSON-RPC message always serialises")
}

/// Decodes one line.
pub fn decode(line: &str) -> Result<Message, DecodeError> {
    let wire: Incoming = serde_json::from_str(line).map_err(DecodeError::Json)?;
    if wire.jsonrpc != VERSION {
        return Err(DecodeError::Invalid("jsonrpc must be \"2.0\""));
    }
    let params = wire.params.unwrap_or(Value::Null);
    match (wire.method, wire.id) {
        (Some(method), Some(id)) => Ok(Message::Request { id, method, params }),
        (Some(method), None) => Ok(Message::Notification { method, params }),
        (None, Some(id)) => {
            let result = match (wire.result, wire.error) {
                (_, Some(error)) => Err(error),
                (Some(value), None) => Ok(value),
                // serde reads `"result": null` as absent, and null is a valid result.
                (None, None) => Ok(Value::Null),
            };
            Ok(Message::Response { id, result })
        }
        (None, None) => Err(DecodeError::Invalid("needs a method or an id")),
    }
}

/// The longest request line a server reads from its client: an engine reading SDK requests and
/// the MCP server reading a host's. A request is a few kilobytes in practice (a storage state
/// is the largest thing in one), so the limit exists to stop a peer that never sends a newline
/// from using all the memory.
pub const MAX_REQUEST_LINE: usize = 8 * 1024 * 1024;

/// What [`read_line_bounded`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// The input ended before any byte of a new line.
    Eof,
    /// A whole line is in the buffer, without its line ending.
    Complete,
    /// The line was longer than the limit. The buffer is empty. With [`read_line_bounded`] the
    /// rest of the line was skipped and the next call starts at the next line.
    TooLong,
}

/// Reads one line of at most `max` bytes (without its `\n` or `\r\n`) into `buf`, which is
/// cleared first. Unlike `BufRead::read_line` and `lines()`, a peer that never sends a
/// newline cannot make this grow memory: past `max` the bytes are read and dropped until
/// the newline, and [`Line::TooLong`] is returned. A last line without a newline counts as a line.
pub fn read_line_bounded<R: BufRead>(reader: &mut R, max: usize, buf: &mut Vec<u8>) -> io::Result<Line> {
    read_line_inner(reader, max, buf, true)
}

/// As [`read_line_bounded`], but [`Line::TooLong`] is returned as soon as the limit is passed,
/// with the rest of the line left unread. For a peer that is dropped after such a line, so
/// that one which never sends a newline is noticed at once and not when it stops.
pub fn read_line_or_give_up<R: BufRead>(reader: &mut R, max: usize, buf: &mut Vec<u8>) -> io::Result<Line> {
    read_line_inner(reader, max, buf, false)
}

fn read_line_inner<R: BufRead>(reader: &mut R, max: usize, buf: &mut Vec<u8>, skip_rest: bool) -> io::Result<Line> {
    buf.clear();
    let mut seen_any = false;
    let mut too_long = false;
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            if !seen_any {
                return Ok(Line::Eof);
            }
            break;
        }
        seen_any = true;
        let newline = chunk.iter().position(|&byte| byte == b'\n');
        let take = newline.unwrap_or(chunk.len());
        if !too_long {
            if buf.len() + take > max {
                if !skip_rest {
                    *buf = Vec::new();
                    return Ok(Line::TooLong);
                }
                too_long = true;
                *buf = Vec::new();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        reader.consume(take + usize::from(newline.is_some()));
        if newline.is_some() {
            break;
        }
    }
    if too_long {
        return Ok(Line::TooLong);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Line::Complete)
}

/// Where a server writes its messages: one line each, and never two lines mixed, whichever
/// thread writes. Cheap to clone, so every thread of a server holds one.
#[derive(Clone)]
pub struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(writer)))
    }

    /// Writes `message` as one line. A peer that has gone away cannot be told anything, so a
    /// failed write is dropped.
    pub fn send(&self, message: &Message) {
        let mut writer = self.0.lock_ok();
        let _ = writeln!(writer, "{}", encode(message)).and_then(|()| writer.flush());
    }

    /// Sends a notification whose parameters are `params`.
    pub fn notify(&self, method: &str, params: &impl Serialize) {
        let params = serde_json::to_value(params).expect("spec types always serialise");
        self.send(&Message::Notification {
            method: method.to_string(),
            params,
        });
    }
}

/// Reads the messages a peer sends, one per line, and hands each to `dispatch`. What cannot be
/// handed on is answered here with an error and an id of `null`, and reading goes on: a line
/// longer than [`MAX_REQUEST_LINE`] (skipped, so one huge line costs no memory), a line that is
/// not UTF-8, and a line that is not a JSON-RPC message. Blank lines are skipped. `refuse`
/// builds the error for a code and a message.
///
/// Returns at the end of `input`, or with the error that ended the reading. The caller then
/// waits for the work it started.
pub fn serve_lines<R: BufRead>(
    mut input: R,
    output: &Output,
    refuse: impl Fn(i64, String) -> RpcError,
    mut dispatch: impl FnMut(Message),
) -> io::Result<()> {
    let refuse_line = |rpc_code: i64, message: String| {
        output.send(&Message::Response {
            id: Id::Null,
            result: Err(refuse(rpc_code, message)),
        });
    };
    let mut buf = Vec::new();
    loop {
        // Lines are read with a limit: a peer that never sends a newline cannot use all the memory.
        match read_line_bounded(&mut input, MAX_REQUEST_LINE, &mut buf)? {
            Line::Eof => return Ok(()),
            Line::Complete => {}
            Line::TooLong => {
                let message = format!(
                    "request larger than {} MiB was skipped",
                    MAX_REQUEST_LINE / (1024 * 1024)
                );
                refuse_line(code::INVALID_REQUEST, message);
                continue;
            }
        }
        // A line that is not UTF-8 is a bad request, not a reason to stop serving.
        let line = match std::str::from_utf8(&buf) {
            Ok(line) => line,
            Err(e) => {
                refuse_line(code::PARSE_ERROR, format!("the request is not valid UTF-8: {e}"));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match decode(line) {
            Ok(message) => dispatch(message),
            Err(e) => refuse_line(code::PARSE_ERROR, e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn lines_of(input: &[u8], max: usize) -> Vec<(Line, Vec<u8>)> {
        let mut reader = Cursor::new(input.to_vec());
        let mut out = Vec::new();
        let mut buf = Vec::new();
        loop {
            let line = read_line_bounded(&mut reader, max, &mut buf).unwrap();
            if line == Line::Eof {
                return out;
            }
            out.push((line, buf.clone()));
        }
    }

    #[test]
    fn read_line_bounded_returns_lines_and_eof() {
        let got = lines_of(b"one\r\ntwo\n\nlast", 100);
        let text: Vec<(Line, &str)> = got
            .iter()
            .map(|(line, bytes)| (*line, std::str::from_utf8(bytes).unwrap()))
            .collect();
        assert_eq!(
            text,
            [
                (Line::Complete, "one"),
                (Line::Complete, "two"),
                (Line::Complete, ""),
                (Line::Complete, "last")
            ]
        );
        assert!(lines_of(b"", 10).is_empty());
    }

    #[test]
    fn read_line_bounded_reports_too_long_and_resumes_at_the_next_line() {
        let got = lines_of(b"aaaa\nok\n", 3);
        assert_eq!(got, [(Line::TooLong, vec![]), (Line::Complete, b"ok".to_vec())]);
        // Exactly at the limit is fine; a long line at the very end, without newline, is too long.
        assert_eq!(lines_of(b"abc\n", 3), [(Line::Complete, b"abc".to_vec())]);
        assert_eq!(lines_of(b"abcd", 3), [(Line::TooLong, vec![])]);
        // The cut can fall between two reads of a small buffer.
        let mut reader = std::io::BufReader::with_capacity(2, Cursor::new(b"abcdefgh\nxy\n".to_vec()));
        let mut buf = Vec::new();
        assert_eq!(read_line_bounded(&mut reader, 3, &mut buf).unwrap(), Line::TooLong);
        assert_eq!(read_line_bounded(&mut reader, 3, &mut buf).unwrap(), Line::Complete);
        assert_eq!(buf, b"xy");
    }

    /// Yields `'a'` for as long as asked, without a newline, and counts what it gave.
    struct Endless {
        given: u64,
        limit: u64,
    }

    impl io::Read for Endless {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = out.len().min((self.limit - self.given) as usize);
            out[..n].fill(b'a');
            self.given += n as u64;
            Ok(n)
        }
    }

    #[test]
    fn read_line_or_give_up_reports_at_once_without_reading_the_rest() {
        let mut reader = std::io::BufReader::with_capacity(
            8192,
            Endless {
                given: 0,
                limit: 100_000_000,
            },
        );
        let mut buf = Vec::new();
        assert_eq!(
            read_line_or_give_up(&mut reader, 1000, &mut buf).unwrap(),
            Line::TooLong
        );
        assert!(reader.get_ref().given <= 8192, "read {} bytes", reader.get_ref().given);
        assert!(buf.is_empty());
    }

    #[test]
    fn read_line_bounded_never_buffers_more_than_max() {
        let mut reader = std::io::BufReader::with_capacity(
            8192,
            Endless {
                given: 0,
                limit: 100_000_000,
            },
        );
        let mut buf = Vec::new();
        assert_eq!(read_line_bounded(&mut reader, 1000, &mut buf).unwrap(), Line::TooLong);
        assert!(buf.capacity() <= 2048, "capacity grew to {}", buf.capacity());
        assert!(buf.is_empty());
        assert_eq!(
            reader.get_ref().given,
            100_000_000,
            "the whole line was read and dropped"
        );
    }

    /// Collects what a server writes.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock_ok().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn written(captured: &Captured) -> Vec<Value> {
        let bytes = captured.0.lock_ok().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn plain(rpc_code: i64, message: String) -> RpcError {
        RpcError {
            code: rpc_code,
            message,
            data: None,
        }
    }

    #[test]
    fn line_server_dispatches_and_joins_threads_at_eof() {
        let captured = Captured::default();
        let output = Output::new(Box::new(captured.clone()));
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"a\"}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"note\"}\n";
        let mut threads = Vec::new();
        let mut seen = Vec::new();
        serve_lines(input.as_bytes(), &output, plain, |message| match message {
            Message::Request { id, method, .. } => {
                let output = output.clone();
                threads.push(std::thread::spawn(move || {
                    output.send(&Message::Response {
                        id,
                        result: Ok(json!({ "method": method })),
                    });
                }));
            }
            other => seen.push(other),
        })
        .unwrap();
        // At the end of the input the caller waits for what it started.
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(seen.len(), 1);
        assert!(matches!(&seen[0], Message::Notification { method, .. } if method == "note"));
        let replies = written(&captured);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[0]["result"]["method"], "a");
    }

    #[test]
    fn oversized_line_still_answers_with_an_error_and_continues() {
        let mut input = vec![b'a'; MAX_REQUEST_LINE + 1];
        input.push(b'\n');
        input.extend_from_slice(b"\xff\xfe not text\nnot json\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n");
        let captured = Captured::default();
        let output = Output::new(Box::new(captured.clone()));
        let mut dispatched = Vec::new();
        serve_lines(input.as_slice(), &output, plain, |message| dispatched.push(message)).unwrap();
        let replies = written(&captured);
        assert_eq!(replies.len(), 3);
        assert!(replies.iter().all(|reply| reply["id"].is_null()));
        assert_eq!(replies[0]["error"]["code"], code::INVALID_REQUEST);
        assert!(replies[0]["error"]["message"].as_str().unwrap().contains("8 MiB"));
        assert_eq!(replies[1]["error"]["code"], code::PARSE_ERROR);
        assert_eq!(replies[2]["error"]["code"], code::PARSE_ERROR);
        // The request after all of that still arrives.
        assert!(matches!(&dispatched[..], [Message::Request { id: Id::Number(7), .. }]));
    }

    #[test]
    fn a_read_error_ends_the_server_with_that_error() {
        struct Broken;
        impl io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("pipe broke"))
            }
        }
        let output = Output::new(Box::new(Captured::default()));
        let error = serve_lines(io::BufReader::new(Broken), &output, plain, |_| {}).unwrap_err();
        assert_eq!(error.to_string(), "pipe broke");
    }

    fn round_trip(message: &Message) {
        let line = encode(message);
        assert!(!line.contains('\n'));
        assert_eq!(&decode(&line).unwrap(), message);
    }

    #[test]
    fn every_kind_round_trips() {
        round_trip(&Message::Request {
            id: Id::Number(7),
            method: "page.goto".into(),
            params: json!({"sessionId": "s1", "url": "https://shop.test/"}),
        });
        round_trip(&Message::Notification {
            method: "driver.log".into(),
            params: json!({"level": "info", "message": "ready"}),
        });
        round_trip(&Message::Response {
            id: Id::String("a".into()),
            result: Ok(json!({"ok": true})),
        });
        round_trip(&Message::Response {
            id: Id::Number(1),
            result: Ok(Value::Null),
        });
        round_trip(&Message::Response {
            id: Id::Number(2),
            result: Err(RpcError {
                code: code::APPLICATION_ERROR,
                message: "navigation timed out".into(),
                data: Some(json!({"kind": "timeout", "retryable": true})),
            }),
        });
    }

    #[test]
    fn decodes_what_other_languages_send() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"driver.shutdown"}"#;
        assert_eq!(
            decode(line).unwrap(),
            Message::Request {
                id: Id::Number(1),
                method: "driver.shutdown".into(),
                params: Value::Null
            }
        );
    }

    #[test]
    fn an_unreadable_request_is_answered_with_a_null_id() {
        let error = RpcError {
            code: code::PARSE_ERROR,
            message: "not valid JSON".into(),
            data: None,
        };
        let line = encode(&Message::Response {
            id: Id::Null,
            result: Err(error),
        });
        assert!(line.contains(r#""id":null"#), "{line}");
    }

    #[test]
    fn rejects_what_is_not_json_rpc() {
        assert!(matches!(decode("not json"), Err(DecodeError::Json(_))));
        assert!(matches!(
            decode(r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#),
            Err(DecodeError::Invalid(_))
        ));
        assert!(matches!(decode(r#"{"jsonrpc":"2.0"}"#), Err(DecodeError::Invalid(_))));
    }
}
