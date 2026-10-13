//! The contracts every part of TraceQA agrees on.
//!
//! [`types`] and [`contracts`] are generated at build time from the JSON
//! Schemas in `spec/`, which are the single source of truth. [`jsonrpc`] is
//! the line format those types travel in.

pub mod jsonrpc;

/// Every type defined in `spec/*.schema.json`.
#[allow(clippy::all)]
pub mod types {
    include!(concat!(env!("OUT_DIR"), "/spec_types.rs"));
}

/// Contract versions and method names.
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
}
