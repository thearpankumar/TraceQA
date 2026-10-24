//! The contracts every part of TraceQA agrees on.
//!
//! [`types`] and [`contracts`] are generated at build time from the JSON
//! Schemas in `spec/`, which are the single source of truth. [`jsonrpc`] is
//! the line format those types travel in.

pub mod error;
pub mod jsonrpc;
pub mod supervise;
pub mod sync;

pub use error::{ErrorKind, ParseError};

/// Every type defined in `spec/*.schema.json`.
#[allow(clippy::all, clippy::pedantic)]
pub mod types {
    include!(concat!(env!("OUT_DIR"), "/spec_types.rs"));
}

/// Contract versions and method names.
#[allow(clippy::pedantic)]
pub mod contracts {
    include!(concat!(env!("OUT_DIR"), "/spec_meta.rs"));
}

impl types::BrowserTarget {
    /// A target that only names its browser.
    pub fn named(browser: impl Into<String>) -> Self {
        Self {
            browser: browser.into(),
            cdp_endpoint: None,
            headless: None,
            webdriver_url: None,
            webdriver_capabilities: Default::default(),
        }
    }

    /// Reads the spellings every front end accepts: a browser name, `cdp=ENDPOINT` to attach
    /// to a running browser, or `webdriver=URL` for a WebDriver server or cloud grid.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let (browser, endpoint) = match text.split_once('=') {
            Some((browser, endpoint)) => (browser, Some(endpoint.to_string())),
            None => (text, None),
        };
        if browser.is_empty() || endpoint.as_deref() == Some("") {
            return Err(ParseError::BrowserTarget(text.to_string()));
        }
        let mut target = Self::named(browser);
        if browser == "webdriver" {
            target.webdriver_url = endpoint;
        } else {
            target.cdp_endpoint = endpoint;
        }
        Ok(target)
    }
}

impl types::Viewport {
    /// Reads `WIDTHxHEIGHT`, such as `1280x800`.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let size = text
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
        match size {
            Some((width, height)) if width > 0 && height > 0 => Ok(Self { height, width }),
            _ => Err(ParseError::Viewport(text.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::types::{BrowserTarget, Viewport};

    #[test]
    fn viewport_needs_two_positive_numbers() {
        assert_eq!(
            Viewport::parse("390x844"),
            Ok(Viewport {
                width: 390,
                height: 844
            })
        );
        for bad in ["1280", "0x800", "ax b", "1280x", ""] {
            assert!(Viewport::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn browser_may_carry_a_cdp_endpoint_or_a_webdriver_url() {
        assert_eq!(BrowserTarget::parse("firefox").unwrap().cdp_endpoint, None);
        let attached = BrowserTarget::parse("cdp=ws://127.0.0.1:9222/devtools/browser/abc?x=1").unwrap();
        assert_eq!(attached.browser, "cdp");
        assert_eq!(
            attached.cdp_endpoint.as_deref(),
            Some("ws://127.0.0.1:9222/devtools/browser/abc?x=1")
        );
        let remote = BrowserTarget::parse("webdriver=http://127.0.0.1:9515").unwrap();
        assert_eq!(
            (remote.cdp_endpoint, remote.webdriver_url.as_deref()),
            (None, Some("http://127.0.0.1:9515"))
        );
        assert!(BrowserTarget::parse("cdp=").is_err() && BrowserTarget::parse("=ws://x").is_err());
    }
}
