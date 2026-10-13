//! The TraceQA engine.
//!
//! All decisions are made here. The engine never names a browser: it talks to
//! drivers through the driver protocol and adapts to the capabilities they
//! report.

pub mod driver;
mod extract;
pub mod fingerprint;
pub mod group;
pub mod run;
pub mod serve;

pub use run::{driver_commands, EngineError, Run};

/// Version reported to clients and drivers, and written into run folders.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The in-page script that lists a page's interactive elements.
pub const COLLECTOR: &str = include_str!("../../../collector/collector.js");
