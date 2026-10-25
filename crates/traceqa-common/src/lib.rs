//! The shared base of every TraceQA crate: the contracts every part agrees on, and the helpers
//! they share ([`locator`], [`supervise`], [`sync`]).
//!
//! [`types`] and [`contracts`] are generated at build time from the JSON
//! Schemas in `spec/`, which are the single source of truth. [`jsonrpc`] is
//! the line format those types travel in.

pub mod error;
pub mod jsonrpc;
pub mod locator;
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

/// The browser names the default drivers open (Playwright: the first five; the WebDriver
/// driver: `safari` and `webdriver`). A driver the person configures may open others.
pub const KNOWN_BROWSERS: [&str; 8] = [
    "chromium",
    "chrome",
    "msedge",
    "firefox",
    "webkit",
    "safari",
    "cdp",
    "webdriver",
];

/// The largest width or height a viewport may have. Larger pages are not a viewport any
/// browser can usefully show, and a huge one only makes a run wait for its timeout.
pub const MAX_VIEWPORT_SIDE: u64 = 10_000;

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

    /// Fails when the browser is not one the default drivers open. Only for front ends that
    /// know no custom driver is in use.
    pub fn check_known(&self) -> Result<(), ParseError> {
        if KNOWN_BROWSERS.contains(&self.browser.as_str()) {
            Ok(())
        } else {
            Err(ParseError::UnknownBrowser(self.browser.clone()))
        }
    }
}

impl types::Viewport {
    /// Reads `WIDTHxHEIGHT`, such as `1280x800`. The sides are checked with [`check`](Self::check).
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let size = text
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
        let Some((width, height)) = size else {
            return Err(ParseError::Viewport(text.to_string()));
        };
        let viewport = Self { height, width };
        viewport.check().map(|()| viewport).map_err(|_| {
            if width.max(height) > MAX_VIEWPORT_SIDE {
                ParseError::ViewportSize(text.to_string())
            } else {
                ParseError::Viewport(text.to_string())
            }
        })
    }

    /// Fails unless each side is between 1 and [`MAX_VIEWPORT_SIDE`]. The one rule for every way a
    /// viewport arrives: the command line, MCP, `run.start` and the SDKs. The schema states the
    /// same range, and a test keeps the two equal.
    pub fn check(&self) -> Result<(), ParseError> {
        let fits = |side: u64| (1..=MAX_VIEWPORT_SIDE).contains(&side);
        if fits(self.width) && fits(self.height) {
            Ok(())
        } else {
            Err(ParseError::ViewportRange {
                width: self.width,
                height: self.height,
            })
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
    fn viewport_check_accepts_1_and_10000_and_rejects_0_and_10001() {
        let check = |width, height| Viewport { width, height }.check();
        assert!(check(1, 1).is_ok() && check(10_000, 10_000).is_ok());
        for (width, height) in [(0, 800), (800, 0), (10_001, 800), (800, 10_001), (99_999, 99_999)] {
            let error = check(width, height).unwrap_err().to_string();
            assert_eq!(
                error,
                format!("viewport {width}x{height}: each side must be between 1 and 10000")
            );
        }
    }

    #[test]
    fn the_schema_bounds_are_the_constant() {
        // The Rust types keep plain u64 (build.rs strips the bounds before generating them), so
        // this is what keeps the schema, which the SDK types are generated from, honest.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/common.schema.json");
        let schema: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for side in ["width", "height"] {
            let property = &schema["$defs"]["Viewport"]["properties"][side];
            assert_eq!(property["minimum"], 1, "{side}");
            assert_eq!(property["maximum"], super::MAX_VIEWPORT_SIDE, "{side}");
        }
    }

    #[test]
    fn viewport_rejects_zero_and_huge_sides() {
        assert!(Viewport::parse("10000x10000").is_ok());
        for bad in ["0x0", "99999x99999", "10001x800", "800x10001"] {
            assert!(Viewport::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        let message = Viewport::parse("99999x99999").unwrap_err().to_string();
        assert!(
            message.starts_with("viewport sides must be between 1 and 10000"),
            "{message}"
        );
    }

    #[test]
    fn browser_known_list() {
        for name in super::KNOWN_BROWSERS {
            assert!(BrowserTarget::named(name).check_known().is_ok(), "{name}");
        }
        let error = BrowserTarget::named("netscape").check_known().unwrap_err().to_string();
        assert!(
            error.starts_with("unknown browser \"netscape\". Known: chromium,"),
            "{error}"
        );
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
