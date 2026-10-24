//! Typed errors shared by the engine, the CLI and the MCP server.
//!
//! On the wire an error kind is the string in `RpcErrorData.kind`. [`ErrorKind`] names the
//! known ones and keeps any other string as it came, so a driver that reports a new kind never
//! breaks the engine.

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// What went wrong, as `RpcErrorData.kind` says it. The wire string of every variant is its
/// snake_case name; `Other` keeps the string of a kind this version does not know.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The browser or driver did not answer in time.
    Timeout,
    NavigationFailed,
    Unsupported,
    SessionNotFound,
    EvalFailed,
    VersionMismatch,
    InvalidParams,
    InvalidRequest,
    /// The page led to a place the operator's rules forbid.
    BlockedUrl,
    /// The server is already running as many calls as it allows.
    Busy,
    Internal,
    /// The driver could not be reached: it did not start, or its pipe broke. The engine's own
    /// kind; drivers do not send it.
    Transport,
    Other(String),
}

impl ErrorKind {
    /// The string used on the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Timeout => "timeout",
            Self::NavigationFailed => "navigation_failed",
            Self::Unsupported => "unsupported",
            Self::SessionNotFound => "session_not_found",
            Self::EvalFailed => "eval_failed",
            Self::VersionMismatch => "version_mismatch",
            Self::InvalidParams => "invalid_params",
            Self::InvalidRequest => "invalid_request",
            Self::BlockedUrl => "blocked_url",
            Self::Busy => "busy",
            Self::Internal => "internal",
            Self::Transport => "transport",
            Self::Other(kind) => kind,
        }
    }

    /// True for the kinds a caller can expect to succeed if it simply tries again later.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout | Self::Busy)
    }
}

impl From<&str> for ErrorKind {
    fn from(kind: &str) -> Self {
        match kind {
            "timeout" => Self::Timeout,
            "navigation_failed" => Self::NavigationFailed,
            "unsupported" => Self::Unsupported,
            "session_not_found" => Self::SessionNotFound,
            "eval_failed" => Self::EvalFailed,
            "version_mismatch" => Self::VersionMismatch,
            "invalid_params" => Self::InvalidParams,
            "invalid_request" => Self::InvalidRequest,
            "blocked_url" => Self::BlockedUrl,
            "busy" => Self::Busy,
            "internal" => Self::Internal,
            "transport" => Self::Transport,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<String> for ErrorKind {
    fn from(kind: String) -> Self {
        Self::from(kind.as_str())
    }
}

impl FromStr for ErrorKind {
    type Err = Infallible;

    fn from_str(kind: &str) -> Result<Self, Infallible> {
        Ok(Self::from(kind))
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

/// Text a front end was given that is not what it asked for. The message is what a person
/// should read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ParseError {
    #[error("expected a browser name, cdp=ENDPOINT or webdriver=URL, got {0:?}")]
    BrowserTarget(String),
    #[error("expected WIDTHxHEIGHT such as 1280x800, got {0:?}")]
    Viewport(String),
    #[error("viewport sides must be between 1 and {max}, got {0:?}", max = crate::MAX_VIEWPORT_SIDE)]
    ViewportSize(String),
    #[error("unknown browser {0:?}. Known: {known}", known = crate::KNOWN_BROWSERS.join(", "))]
    UnknownBrowser(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: [(&str, ErrorKind); 12] = [
        ("timeout", ErrorKind::Timeout),
        ("navigation_failed", ErrorKind::NavigationFailed),
        ("unsupported", ErrorKind::Unsupported),
        ("session_not_found", ErrorKind::SessionNotFound),
        ("eval_failed", ErrorKind::EvalFailed),
        ("version_mismatch", ErrorKind::VersionMismatch),
        ("invalid_params", ErrorKind::InvalidParams),
        ("invalid_request", ErrorKind::InvalidRequest),
        ("blocked_url", ErrorKind::BlockedUrl),
        ("busy", ErrorKind::Busy),
        ("internal", ErrorKind::Internal),
        ("transport", ErrorKind::Transport),
    ];

    #[test]
    fn wire_strings_round_trip_for_every_known_kind() {
        for (wire, kind) in KNOWN {
            assert_eq!(kind.as_str(), wire);
            assert_eq!(ErrorKind::from(wire), kind);
            assert_eq!(wire.parse::<ErrorKind>().unwrap(), kind);
            assert_eq!(kind.to_string(), wire);
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(serde_json::from_str::<ErrorKind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn every_kind_the_spec_lists_is_known() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../spec/driver-protocol.schema.json")).unwrap();
        let description = schema["$defs"]["RpcErrorData"]["properties"]["kind"]["description"]
            .as_str()
            .expect("the spec lists the known kinds");
        let listed = description
            .trim_start_matches("Known values: ")
            .trim_end_matches('.')
            .split(", ");
        for wire in listed {
            assert!(
                !matches!(ErrorKind::from(wire), ErrorKind::Other(_)),
                "{wire} is in the spec but not in ErrorKind"
            );
        }
    }

    #[test]
    fn unknown_kinds_become_other_and_serialise_back_unchanged() {
        let kind: ErrorKind = serde_json::from_str("\"rate_limited\"").unwrap();
        assert_eq!(kind, ErrorKind::Other("rate_limited".into()));
        assert_eq!(serde_json::to_string(&kind).unwrap(), "\"rate_limited\"");
        assert_eq!(kind.to_string(), "rate_limited");
    }

    #[test]
    fn only_timeouts_and_busy_are_worth_retrying() {
        for (_, kind) in KNOWN {
            let expected = matches!(kind, ErrorKind::Timeout | ErrorKind::Busy);
            assert_eq!(kind.is_retryable(), expected, "{kind}");
        }
        assert!(!ErrorKind::Other("timeout2".into()).is_retryable());
    }

    #[test]
    fn parse_errors_are_typed() {
        use crate::types::{BrowserTarget, Viewport};
        assert_eq!(
            Viewport::parse("1280").unwrap_err(),
            ParseError::Viewport("1280".into())
        );
        assert_eq!(
            BrowserTarget::parse("cdp=").unwrap_err(),
            ParseError::BrowserTarget("cdp=".into())
        );
        // The text a person reads is what the String errors said before.
        assert_eq!(
            Viewport::parse("0x800").unwrap_err().to_string(),
            "expected WIDTHxHEIGHT such as 1280x800, got \"0x800\""
        );
        assert_eq!(
            BrowserTarget::parse("=x").unwrap_err().to_string(),
            "expected a browser name, cdp=ENDPOINT or webdriver=URL, got \"=x\""
        );
    }
}
