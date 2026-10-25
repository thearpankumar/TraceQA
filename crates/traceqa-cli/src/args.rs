//! Arguments that more than one command takes.

use clap::Args;
use traceqa_protocol::types::{BrowserTarget, Viewport};
use traceqa_protocol::ParseError;

#[derive(Args)]
pub struct DriverArgs {
    /// Command that starts a browser driver. Repeat for several. Default: $TRACEQA_DRIVERS,
    /// then traceqa-driver-playwright and traceqa-driver-webdriver from PATH.
    #[arg(long = "driver", value_name = "COMMAND")]
    pub drivers: Vec<String>,
}

pub fn parse_viewport(text: &str) -> Result<Viewport, ParseError> {
    Viewport::parse(text)
}

pub fn parse_browser(text: &str) -> Result<BrowserTarget, ParseError> {
    BrowserTarget::parse(text)
}
