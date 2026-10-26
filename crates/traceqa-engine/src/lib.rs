//! The TraceQA engine.
//!
//! All decisions are made here. The engine never names a browser: it talks to
//! drivers through the driver protocol and adapts to the capabilities they
//! report.

//!
//! The public surface is the driver side ([`driver`]), the run ([`Run`], [`Execution`]), the
//! servers ([`serve`]), the operator's rules ([`UrlPolicy`]) and the places TraceQA keeps its
//! files ([`home`]). How elements are fingerprinted and grouped is an internal detail, so it
//! can change without a new major version:
//!
//! ```compile_fail,E0603
//! use traceqa_engine::fingerprint::component_id;
//! ```
//!
//! ```compile_fail,E0603
//! use traceqa_engine::group::group;
//! ```
//!
//! Scripted drivers for tests (`driver::Drivers::starting_with`) exist only with the
//! `test-support` feature, which no release build enables.

pub mod driver;
mod extract;
mod fingerprint;
mod group;
pub mod home;
mod locator;
pub mod policy;
pub mod run;
pub mod serve;
pub mod url_policy;
mod url_safety;
mod util;

/// axe-core for `--axe`: where it is installed from and how a run finds it.
pub use extract::axe;
pub use run::{driver_commands, Abort, EngineError, Execution, InFlight, Interrupt, Run};
pub use url_policy::UrlPolicy;
/// Refuses an address a run may not be started on. The MCP server asks before it accepts a call.
pub use url_safety::check_navigable;

/// Version reported to clients and drivers, and written into run folders.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The in-page script that lists a page's interactive elements.
pub const COLLECTOR: &str = include_str!("../../../page-script/page-script.js");
