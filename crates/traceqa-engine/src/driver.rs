//! The engine's side of the driver protocol.
//!
//! [`Driver`] offers one typed method per protocol method. It talks through a
//! [`Transport`], which is a child process in production and a script in tests.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use traceqa_protocol::contracts::driver_protocol::{method, notification, VERSION as PROTOCOL_VERSION};
use traceqa_protocol::jsonrpc::{self, Id, Message};
use traceqa_protocol::types::{
    DriverHelloParams, DriverHelloResult, DriverLogParams, Empty, PageEvalParams, PageEvalResult, PageGotoParams,
    PageGotoResult, PageScreenshotParams, PageScreenshotResult, PageScrollParams, PageScrollResult, RpcErrorData,
    SessionCloseParams, SessionOpenParams, SessionOpenResult, SessionStorageParams, StorageState,
};

/// Time a page load or a script gets when the caller names none.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Time allowed for calls that should be quick. Page loads and scripts pass their own.
const SHORT: Duration = Duration::from_secs(30);
/// Time allowed for the slow calls: starting a browser on a cold machine, and a full-page screenshot.
const SLOW: Duration = Duration::from_secs(90);
/// Added on top of a timeout the driver enforces itself, so the driver reports it first.
const MARGIN: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub struct DriverError {
    /// An `RpcErrorData.kind` from the spec, or `transport` when the driver could not be reached.
    pub kind: String,
    pub message: String,
}

impl DriverError {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            kind: "transport".into(),
            message: message.into(),
        }
    }

    /// True when the driver itself can no longer be trusted: it did not answer in time, or
    /// the pipe to it broke. Its state is unknown, so it must be replaced, not reused.
    fn loses_driver(&self) -> bool {
        self.kind == "timeout" || self.kind == "transport"
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.kind)
    }
}

impl std::error::Error for DriverError {}

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

impl ProcessTransport {
    pub fn spawn(command: &DriverCommand) -> Result<Self, DriverError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| DriverError::transport(format!("could not start `{command}`: {e}")))?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let (sender, incoming) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let message = match line {
                    Ok(line) if line.trim().is_empty() => continue,
                    Ok(line) => jsonrpc::decode(&line).map_err(|e| format!("{e}: {line}")),
                    Err(e) => Err(e.to_string()),
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
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
                    return result.map_err(|error| {
                        let data = error
                            .data
                            .and_then(|data| serde_json::from_value::<RpcErrorData>(data).ok());
                        DriverError {
                            kind: data.map_or_else(|| "internal".to_string(), |d| d.kind),
                            message: error.message,
                        }
                    });
                }
                Ok(Ok(Message::Notification { method, params })) if method == notification::DRIVER_LOG => {
                    if let Ok(log) = serde_json::from_value::<DriverLogParams>(params) {
                        eprintln!("[driver {}] {}", log.level, log.message);
                    }
                }
                // An answer to a request we gave up on, or a notification we do not know.
                Ok(Ok(_)) => {}
                Ok(Err(garbage)) => return Err(DriverError::transport(format!("driver sent a bad line: {garbage}"))),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(DriverError {
                        kind: "timeout".into(),
                        message: format!("driver did not answer {method} within {} s", timeout.as_secs()),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(DriverError::transport(format!("driver exited while handling {method}")));
                }
            }
        }
    }
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
            return Err(DriverError {
                kind: "version_mismatch".into(),
                message: format!(
                    "{} speaks driver protocol {}, this engine speaks {PROTOCOL_VERSION}",
                    info.driver_name, info.protocol_version
                ),
            });
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
    let result = transport.call(method, params, timeout)?;
    serde_json::from_value(result).map_err(|e| DriverError {
        kind: "internal".into(),
        message: format!("driver answered {method} with something the spec does not allow: {e}"),
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
            _ => Err(DriverError {
                kind: "unsupported".into(),
                message: format!("no driver can open {browser:?}. Tried: {}", tried.join("; ")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_missing_driver_is_reported_with_what_was_tried() {
        let mut drivers = Drivers::new(vec![DriverCommand::parse("traceqa-no-such-driver").unwrap()]);
        let error = drivers.for_browser("firefox").err().unwrap();
        assert_eq!(error.kind, "unsupported");
        assert!(error.message.contains("traceqa-no-such-driver"), "{}", error.message);
    }
}
