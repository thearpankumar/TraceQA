//! The engine's side of the driver protocol.
//!
//! [`Driver`] offers one typed method per protocol method. It talks through a
//! [`Transport`], which is a child process in production and a script in tests.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use traceqa_protocol::contracts::driver_protocol::{method, notification, VERSION as PROTOCOL_VERSION};
use traceqa_protocol::jsonrpc::{self, Id, Line, Message, RpcError};
use traceqa_protocol::types::{
    DriverHelloParams, DriverHelloResult, DriverLogParams, Empty, PageEvalParams, PageEvalResult, PageGotoParams,
    PageGotoResult, PageScreenshotParams, PageScreenshotResult, PageScrollParams, PageScrollResult, RpcErrorData,
    SessionCloseParams, SessionOpenParams, SessionOpenResult, SessionStorageParams, StorageState,
};
pub use traceqa_protocol::ErrorKind;

use crate::url_safety::redact_text;

/// Time a page load or a script gets when the caller names none.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Time allowed for calls that should be quick. Page loads and scripts pass their own.
const SHORT: Duration = Duration::from_secs(30);
/// Time allowed for the slow calls: starting a browser on a cold machine, and a full-page screenshot.
const SLOW: Duration = Duration::from_secs(90);
/// Added on top of a timeout the driver enforces itself, so the driver reports it first.
const MARGIN: Duration = Duration::from_secs(15);

/// A request to a driver failed: the driver said so, or could not be reached.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("{message} ({kind})")]
pub struct DriverError {
    kind: ErrorKind,
    message: String,
}

impl DriverError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// An `RpcErrorData.kind` from the spec, or `transport` when the driver could not be reached.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// True when trying again later can be expected to work.
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    fn transport(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transport, redact_text(&message.into()))
    }

    /// The same error with the secret values in any address in its message hidden. Text from a
    /// driver can quote the address it was sent to, and a query can carry a token.
    fn redacted(mut self) -> Self {
        self.message = redact_text(&self.message);
        self
    }

    /// True when the driver itself can no longer be trusted: it did not answer in time, or
    /// the pipe to it broke. Its state is unknown, so it must be replaced, not reused.
    pub(crate) fn loses_driver(&self) -> bool {
        matches!(self.kind, ErrorKind::Timeout | ErrorKind::Transport)
    }
}

/// Carries one request to a driver and brings back its result.
pub trait Transport: Send {
    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, DriverError>;
}

/// How to start a driver: a program and its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct DriverCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl DriverCommand {
    /// Splits a command line into a program and its arguments. Words are separated by
    /// whitespace, and double quotes keep a word with spaces together, as in
    /// `"/Applications/My Tools/driver" --port 9000`. Every other character is taken as it
    /// is, so Windows paths and names with apostrophes need no escaping. Returns `None`
    /// for an empty line.
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut in_word = false;
        let mut quoted = false;
        for c in line.chars() {
            if c == '"' {
                quoted = !quoted;
                in_word = true;
            } else if c.is_whitespace() && !quoted {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            } else {
                word.push(c);
                in_word = true;
            }
        }
        if in_word {
            words.push(word);
        }
        let mut words = words.into_iter();
        Some(Self {
            program: words.next()?,
            args: words.collect(),
        })
    }
}

impl fmt::Display for DriverCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            std::iter::once(&self.program)
                .chain(&self.args)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

/// A driver running as a child process, spoken to over its stdin and stdout.
pub struct ProcessTransport {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Result<Message, String>>,
    next_id: u64,
    /// Set once the driver stopped answering, so dropping it does not wait on it again.
    unresponsive: bool,
}

/// The longest line the engine accepts from a driver. A collector result for a very large
/// page is tens of MiB, so the limit is generous; its job is to stop a driver that never
/// sends a newline from using all the memory. A longer line makes the driver count as lost.
pub const MAX_DRIVER_LINE: usize = 64 * 1024 * 1024;

/// How many messages from the driver may wait to be read. When the engine is busy the
/// reader thread blocks, so the driver's pipe fills and the driver slows down instead of
/// the queue growing.
const QUEUE_LIMIT: usize = 64;

/// Longest piece of a driver's output that goes into an error message.
const EXCERPT_CHARS: usize = 200;

/// At most [`EXCERPT_CHARS`] characters of `bytes`, with control characters shown as escapes,
/// and the size of the whole when it was cut.
fn excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut shown = String::new();
    let mut chars = text.chars();
    for c in chars.by_ref().take(EXCERPT_CHARS) {
        if c.is_control() {
            shown.extend(c.escape_default());
        } else {
            shown.push(c);
        }
    }
    if chars.next().is_some() {
        shown.push_str(&format!("… ({} bytes)", bytes.len()));
    }
    shown
}

/// Reads the driver's lines and queues what they decode to, until the pipe ends, the line
/// limit is passed or the queue is no longer read. `read` counts lines taken from the pipe.
fn read_messages<R: BufRead>(
    mut stdout: R,
    max_line: usize,
    sender: &mpsc::SyncSender<Result<Message, String>>,
    read: &AtomicUsize,
) {
    let mut buf = Vec::new();
    loop {
        // A line that is not text, or an unreadable pipe, ends the reader: nothing after it can be trusted.
        let (message, stop) = match jsonrpc::read_line_or_give_up(&mut stdout, max_line, &mut buf) {
            Ok(Line::Eof) => return,
            Ok(Line::Complete) => match std::str::from_utf8(&buf) {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => (
                    jsonrpc::decode(line).map_err(|e| format!("{e}: {}", excerpt(line.as_bytes()))),
                    false,
                ),
                Err(_) => (Err(format!("not valid UTF-8: {}", excerpt(&buf))), true),
            },
            Ok(Line::TooLong) => (Err(format!("a line longer than {max_line} bytes")), true),
            Err(e) => (Err(e.to_string()), true),
        };
        read.fetch_add(1, Ordering::SeqCst);
        if sender.send(message).is_err() || stop {
            return;
        }
    }
}

impl ProcessTransport {
    pub fn spawn(command: &DriverCommand) -> Result<Self, DriverError> {
        Self::spawn_with_limit(command, MAX_DRIVER_LINE)
    }

    /// As [`spawn`](Self::spawn), with another limit on the length of a line from the driver.
    pub fn spawn_with_limit(command: &DriverCommand, max_line: usize) -> Result<Self, DriverError> {
        let mut process = Command::new(&command.program);
        process.args(&command.args);
        Self::start(process, command, max_line)
    }

    fn start(mut process: Command, command: &DriverCommand, max_line: usize) -> Result<Self, DriverError> {
        // The driver's stderr is its own log channel: it goes straight to ours, never through the engine.
        let mut child = process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| DriverError::transport(format!("could not start `{command}`: {e}")))?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let (sender, incoming) = mpsc::sync_channel(QUEUE_LIMIT);
        thread::spawn(move || read_messages(BufReader::new(stdout), max_line, &sender, &AtomicUsize::new(0)));
        Ok(Self {
            child,
            stdin,
            incoming,
            next_id: 0,
            unresponsive: false,
        })
    }
}

impl Transport for ProcessTransport {
    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, DriverError> {
        let result = self.exchange(method, params, timeout);
        self.unresponsive |= result.as_ref().is_err_and(DriverError::loses_driver);
        result
    }
}

impl ProcessTransport {
    fn exchange(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, DriverError> {
        self.next_id += 1;
        let id = Id::Number(self.next_id);
        let line = jsonrpc::encode(&Message::Request {
            id: id.clone(),
            method: method.to_string(),
            params,
        });
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| DriverError::transport(format!("driver is not reading: {e}")))?;

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.incoming.recv_timeout(remaining) {
                Ok(Ok(Message::Response { id: answered, result })) if answered == id => {
                    return result.map_err(driver_error);
                }
                Ok(Ok(Message::Notification { method, params })) if method == notification::DRIVER_LOG => {
                    if let Ok(log) = serde_json::from_value::<DriverLogParams>(params) {
                        eprintln!("{}", log_line(&log));
                    }
                }
                // An answer to a request we gave up on, or a notification we do not know.
                Ok(Ok(_)) => {}
                Ok(Err(garbage)) => return Err(DriverError::transport(format!("driver sent a bad line: {garbage}"))),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(DriverError::new(
                        ErrorKind::Timeout,
                        format!("driver did not answer {method} within {} s", timeout.as_secs()),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(DriverError::transport(format!("driver exited while handling {method}")));
                }
            }
        }
    }
}

/// What the driver's error response means for the engine. The message is the driver's own text,
/// so any secret in an address it quotes is hidden before it goes anywhere else.
fn driver_error(error: RpcError) -> DriverError {
    let data = error
        .data
        .and_then(|data| serde_json::from_value::<RpcErrorData>(data).ok());
    DriverError::new(
        data.map_or(ErrorKind::Internal, |d| ErrorKind::from(d.kind)),
        error.message,
    )
    .redacted()
}

/// The line the engine prints for a `driver.log` notification, with addresses redacted.
fn log_line(log: &DriverLogParams) -> String {
    redact_text(&format!("[driver {}] {}", log.level, log.message))
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        // Ask politely so the driver can close its browsers, then make sure it is gone.
        if !self.unresponsive {
            let _ = self.call(
                method::DRIVER_SHUTDOWN,
                Value::Object(Default::default()),
                Duration::from_secs(10),
            );
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A connected driver that has completed the version handshake.
pub struct Driver {
    transport: Box<dyn Transport>,
    pub info: DriverHelloResult,
    lost: bool,
}

impl Driver {
    /// Says hello and refuses a driver that speaks another protocol version.
    pub fn connect(mut transport: Box<dyn Transport>) -> Result<Self, DriverError> {
        let hello = DriverHelloParams {
            protocol_version: PROTOCOL_VERSION,
            engine_version: crate::VERSION.to_string(),
        };
        let info: DriverHelloResult = request(transport.as_mut(), method::DRIVER_HELLO, &hello, SHORT)?;
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(DriverError::new(
                ErrorKind::VersionMismatch,
                format!(
                    "{} speaks driver protocol {}, this engine speaks {PROTOCOL_VERSION}",
                    info.driver_name, info.protocol_version
                ),
            ));
        }
        Ok(Self {
            transport,
            info,
            lost: false,
        })
    }

    /// True once the driver timed out or its pipe broke. A lost driver is never used again:
    /// [`Drivers::for_browser`] starts a fresh one in its place.
    pub fn is_lost(&self) -> bool {
        self.lost
    }

    fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
        timeout: Duration,
    ) -> Result<R, DriverError> {
        let result = request(self.transport.as_mut(), method, params, timeout);
        self.lost |= result.as_ref().is_err_and(DriverError::loses_driver);
        result
    }

    pub fn supports(&self, browser: &str) -> bool {
        self.info.browsers.iter().any(|name| name == browser)
    }

    /// True when only one session of `browser` can exist on the machine at a time.
    pub fn is_exclusive(&self, browser: &str) -> bool {
        self.info.exclusive_browsers.iter().any(|name| name == browser)
    }

    pub fn open_session(&mut self, params: &SessionOpenParams) -> Result<SessionOpenResult, DriverError> {
        self.request(method::SESSION_OPEN, params, SLOW)
    }

    pub fn close_session(&mut self, session_id: &str) -> Result<(), DriverError> {
        let params = SessionCloseParams {
            session_id: session_id.to_string(),
        };
        self.request::<_, Empty>(method::SESSION_CLOSE, &params, SHORT)
            .map(drop)
    }

    /// The session's cookies and stored values, to save them after someone has signed in.
    pub fn storage(&mut self, session_id: &str) -> Result<StorageState, DriverError> {
        let params = SessionStorageParams {
            session_id: session_id.to_string(),
        };
        self.request(method::SESSION_STORAGE, &params, SHORT)
    }

    pub fn goto(&mut self, params: &PageGotoParams) -> Result<PageGotoResult, DriverError> {
        let own = Duration::from_millis(params.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
        self.request(method::PAGE_GOTO, params, own + MARGIN)
    }

    pub fn eval(&mut self, params: &PageEvalParams) -> Result<PageEvalResult, DriverError> {
        let own = Duration::from_millis(params.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
        self.request(method::PAGE_EVAL, params, own + MARGIN)
    }

    pub fn scroll(&mut self, params: &PageScrollParams) -> Result<PageScrollResult, DriverError> {
        self.request(method::PAGE_SCROLL, params, SHORT)
    }

    pub fn screenshot(&mut self, params: &PageScreenshotParams) -> Result<PageScreenshotResult, DriverError> {
        self.request(method::PAGE_SCREENSHOT, params, SLOW)
    }
}

fn request<P: Serialize, R: DeserializeOwned>(
    transport: &mut dyn Transport,
    method: &str,
    params: &P,
    timeout: Duration,
) -> Result<R, DriverError> {
    let params = serde_json::to_value(params).expect("spec types always serialise");
    // A transport is trusted with nothing: a scripted or third-party one can hand back an error
    // that quotes an address, so what comes out of it is redacted here as well.
    let result = transport.call(method, params, timeout).map_err(DriverError::redacted)?;
    serde_json::from_value(result).map_err(|e| {
        DriverError::new(
            ErrorKind::Internal,
            format!("driver answered {method} with something the spec does not allow: {e}"),
        )
        .redacted()
    })
}

/// Starts one driver and completes the handshake with it.
pub type Starter = Box<dyn Fn() -> Result<Driver, DriverError> + Send>;

/// The configured drivers. Each is started the first time a browser might need it, and
/// started again if it stops answering.
pub struct Drivers {
    slots: Vec<(Starter, Slot)>,
}

enum Slot {
    NotStarted,
    Running(Driver),
    Failed(String),
}

impl Drivers {
    /// Drivers that are separate programs, started with these commands.
    pub fn new(commands: Vec<DriverCommand>) -> Self {
        let starters = commands.into_iter().map(|command| {
            let start =
                move || ProcessTransport::spawn(&command).and_then(|transport| Driver::connect(Box::new(transport)));
            Box::new(start) as Starter
        });
        Self::starting_with(starters.collect())
    }

    /// Drivers that are started some other way. Tests use this to supply scripted drivers.
    pub fn starting_with(starters: Vec<Starter>) -> Self {
        Self {
            slots: starters.into_iter().map(|start| (start, Slot::NotStarted)).collect(),
        }
    }

    /// Returns the first configured driver that can open `browser`. A driver that was lost
    /// on an earlier call is dropped, which ends its process, and started again.
    pub fn for_browser(&mut self, browser: &str) -> Result<&mut Driver, DriverError> {
        let mut tried = Vec::new();
        let mut found = None;
        for (index, (start, slot)) in self.slots.iter_mut().enumerate() {
            if matches!(slot, Slot::Running(driver) if driver.is_lost()) {
                *slot = Slot::NotStarted;
            }
            if matches!(slot, Slot::NotStarted) {
                *slot = match start() {
                    Ok(driver) => Slot::Running(driver),
                    Err(error) => Slot::Failed(error.message),
                };
            }
            match slot {
                Slot::Running(driver) if driver.supports(browser) => {
                    found = Some(index);
                    break;
                }
                Slot::Running(driver) => tried.push(format!(
                    "{} opens {}",
                    driver.info.driver_name,
                    driver.info.browsers.join(", ")
                )),
                Slot::Failed(why) => tried.push(why.clone()),
                Slot::NotStarted => unreachable!("every slot was started above"),
            }
        }
        match found.map(|index| &mut self.slots[index].1) {
            Some(Slot::Running(driver)) => Ok(driver),
            _ => Err(DriverError::new(
                ErrorKind::Unsupported,
                format!("no driver can open {browser:?}. Tried: {}", tried.join("; ")),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn command_lines_split_on_whitespace_and_respect_quotes() {
        let command = DriverCommand::parse("  python -m  traceqa_driver_playwright ").unwrap();
        assert_eq!(command.program, "python");
        assert_eq!(command.args, ["-m", "traceqa_driver_playwright"]);
        assert_eq!(command.to_string(), "python -m traceqa_driver_playwright");
        assert_eq!(DriverCommand::parse("   "), None);

        let quoted = DriverCommand::parse(r#""/Applications/My Tools/driver" --name "two words" --empty """#).unwrap();
        assert_eq!(quoted.program, "/Applications/My Tools/driver");
        assert_eq!(quoted.args, ["--name", "two words", "--empty", ""]);
        // Backslashes and apostrophes are ordinary characters, so Windows paths pass through untouched.
        let windows = DriverCommand::parse(r#""C:\Program Files\O'Brien\driver.exe" -v"#).unwrap();
        assert_eq!(windows.program, r"C:\Program Files\O'Brien\driver.exe");
        assert_eq!(
            DriverCommand::parse(r"C:\Users\O'Brien\driver.exe -v").unwrap().args,
            ["-v"]
        );
    }

    #[test]
    fn a_driver_error_message_never_carries_secret_query_values() {
        let error = RpcError {
            code: -32000,
            message: "Page.goto: net::ERR_CONNECTION_REFUSED at https://h/cb?token=abc&page=2".into(),
            data: Some(json!({ "kind": "navigation_failed", "retryable": true })),
        };
        let error = driver_error(error);
        assert_eq!(error.kind(), &ErrorKind::NavigationFailed);
        assert_eq!(
            error.message,
            "Page.goto: net::ERR_CONNECTION_REFUSED at https://h/cb?token=REDACTED&page=2"
        );
        assert!(!error.to_string().contains("abc"));

        // A transport that is not ours is trusted no more than the driver process.
        struct Quoting;
        impl Transport for Quoting {
            fn call(&mut self, _: &str, _: Value, _: Duration) -> Result<Value, DriverError> {
                Err(DriverError::new(
                    ErrorKind::NavigationFailed,
                    "failed at http://h/?api_key=abc",
                ))
            }
        }
        let failed = request::<_, Empty>(&mut Quoting, "page.goto", &json!({}), SHORT).unwrap_err();
        assert_eq!(failed.message(), "failed at http://h/?api_key=REDACTED");
        // And so is what the engine itself reports about a driver line that was not valid.
        let garbage = DriverError::transport("driver sent a bad line: {\"u\":\"https://h/?sig=abc\"");
        assert!(!garbage.message().contains("abc"), "{}", garbage.message());
    }

    #[test]
    fn driver_log_lines_are_redacted_before_they_reach_stderr() {
        let log = DriverLogParams {
            level: traceqa_protocol::types::LogLevel::Warn,
            message: "retrying https://h/oauth?code=1&access_token=abc after a reset".into(),
        };
        assert_eq!(
            log_line(&log),
            "[driver warn] retrying https://h/oauth?code=1&access_token=REDACTED after a reset"
        );
    }

    const CHILD_SWITCH: &str = "TRACEQA_TEST_ENDLESS_LINE";

    /// Not a test on its own: the child process of `a_driver_that_prints_an_endless_line...`
    /// runs this one function of the test binary. It prints a 1 MiB line with no newline and
    /// then stays alive, like a driver that has gone wrong.
    #[test]
    fn endless_line_child() {
        if std::env::var_os(CHILD_SWITCH).is_none() {
            return;
        }
        let mut out = std::io::stdout();
        out.write_all(&vec![b'a'; 1 << 20]).unwrap();
        out.flush().unwrap();
        thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn a_driver_that_prints_an_endless_line_is_marked_lost_with_a_short_excerpt() {
        let mut process = Command::new(std::env::current_exe().unwrap());
        process
            .args([
                "--exact",
                "driver::tests::endless_line_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_SWITCH, "1");
        let command = DriverCommand {
            program: "test-binary".into(),
            args: vec![],
        };
        let mut transport = ProcessTransport::start(process, &command, 4096).unwrap();

        // The test harness prints a few lines of its own first. Each is one bad line, answered
        // with an error, until the long one arrives.
        let mut error = None;
        for _ in 0..5 {
            let result = transport.call("page.eval", json!({}), Duration::from_secs(20));
            let failure = result.unwrap_err();
            if failure.message.contains("longer than 4096 bytes") {
                error = Some(failure);
                break;
            }
        }
        let error = error.expect("the long line was reported");
        assert_eq!(error.kind(), &ErrorKind::Transport);
        assert!(error.loses_driver());
        assert!(
            error.message.len() < 400,
            "{} bytes: {}",
            error.message.len(),
            error.message
        );
        assert!(transport.unresponsive, "the driver is marked lost");

        let started = Instant::now();
        drop(transport);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "dropping a busy transport must not wait"
        );
    }

    #[test]
    fn garbage_line_error_shows_at_most_200_characters() {
        let garbage = format!("{}\t{}", "x".repeat(150), "y".repeat(5000));
        let shown = excerpt(garbage.as_bytes());
        assert!(shown.starts_with(&"x".repeat(150)));
        assert!(shown.contains("\\t"), "control characters are escaped: {shown}");
        assert!(shown.ends_with("… (5151 bytes)"), "{shown}");
        assert!(shown.chars().count() < 200 + 20, "{}", shown.chars().count());
        assert_eq!(excerpt(b"short"), "short");

        // Through the reader, a bad line gives an error with the same bound.
        let (sender, receiver) = mpsc::sync_channel(4);
        let input = format!("{garbage}\n");
        read_messages(input.as_bytes(), 1 << 20, &sender, &AtomicUsize::new(0));
        let message = receiver.recv().unwrap().unwrap_err();
        assert!(message.chars().count() < 300, "{message}");
    }

    #[test]
    fn a_line_that_is_not_text_ends_the_reader() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let input: &[u8] = b"\xff\xfe not text\n{\"jsonrpc\":\"2.0\",\"method\":\"driver.log\"}\n";
        read_messages(input, 1 << 20, &sender, &AtomicUsize::new(0));
        drop(sender);
        assert!(receiver.recv().unwrap().unwrap_err().starts_with("not valid UTF-8"));
        assert!(receiver.recv().is_err(), "nothing after the bad line is read");
    }

    #[test]
    fn the_channel_applies_backpressure() {
        let notification = "{\"jsonrpc\":\"2.0\",\"method\":\"driver.log\",\"params\":{}}\n";
        let input = notification.repeat(10_000);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_LIMIT);
        let read = Arc::new(AtomicUsize::new(0));
        let reader = {
            let read = Arc::clone(&read);
            thread::spawn(move || read_messages(input.as_bytes(), 1 << 20, &sender, &read))
        };
        // Nobody reads the queue: the reader may take a full queue and the one it is holding, no more.
        thread::sleep(Duration::from_millis(300));
        let waiting = read.load(Ordering::SeqCst);
        assert!(waiting <= QUEUE_LIMIT + 1, "the reader ran ahead by {waiting} lines");
        assert!(!reader.is_finished(), "the reader must be blocked, not done");

        // Once the queue is read, everything arrives.
        assert_eq!(receiver.iter().count(), 10_000);
        reader.join().unwrap();

        // A reader blocked on a full queue ends when the queue is dropped.
        let (sender, receiver) = mpsc::sync_channel(1);
        let input = notification.repeat(10);
        let reader = thread::spawn(move || read_messages(input.as_bytes(), 1 << 20, &sender, &AtomicUsize::new(0)));
        thread::sleep(Duration::from_millis(100));
        drop(receiver);
        reader.join().unwrap();
    }

    #[test]
    fn a_missing_driver_is_reported_with_what_was_tried() {
        let mut drivers = Drivers::new(vec![DriverCommand::parse("traceqa-no-such-driver").unwrap()]);
        let error = drivers.for_browser("firefox").err().unwrap();
        assert_eq!(error.kind(), &ErrorKind::Unsupported);
        assert!(error.message.contains("traceqa-no-such-driver"), "{}", error.message);
    }

    #[test]
    fn loses_driver_only_for_timeout_and_transport() {
        let loses = |kind: ErrorKind| DriverError::new(kind, "x").loses_driver();
        assert!(loses(ErrorKind::Timeout));
        assert!(loses(ErrorKind::Transport));
        for kind in [
            ErrorKind::NavigationFailed,
            ErrorKind::Unsupported,
            ErrorKind::SessionNotFound,
            ErrorKind::EvalFailed,
            ErrorKind::VersionMismatch,
            ErrorKind::InvalidParams,
            ErrorKind::InvalidRequest,
            ErrorKind::BlockedUrl,
            ErrorKind::Busy,
            ErrorKind::Internal,
            ErrorKind::Other("timeout2".into()),
        ] {
            assert!(!loses(kind.clone()), "{kind}");
        }
    }

    #[test]
    fn driver_errors_read_as_they_always_did() {
        let error = DriverError::new(ErrorKind::Timeout, "driver did not answer page.goto within 30 s");
        assert_eq!(
            error.to_string(),
            "driver did not answer page.goto within 30 s (timeout)"
        );
        assert_eq!(
            (error.kind(), error.message()),
            (&ErrorKind::Timeout, "driver did not answer page.goto within 30 s")
        );
        assert!(error.is_retryable());
        assert!(!DriverError::new(ErrorKind::Unsupported, "x").is_retryable());
    }
}
