//! Arguments that more than one command takes.

use clap::{Args, ValueEnum};
use traceqa_common::types::{BrowserTarget, Viewport};
use traceqa_common::ParseError;
use traceqa_engine::PageRead;

#[derive(Args)]
pub struct DriverArgs {
    /// Command that starts a browser driver. Repeat for several. Default: $TRACEQA_DRIVERS,
    /// then traceqa-driver-playwright and traceqa-driver-webdriver from PATH.
    #[arg(long = "driver", value_name = "COMMAND")]
    pub drivers: Vec<String>,
}

/// How the page is carried out of the browser after each action (`--page-read`).
#[derive(Clone, Copy, ValueEnum)]
pub enum PageReadArg {
    /// As JSON text written inside the page: the same page, many times faster on a big one. The default.
    Fast,
    /// As the browser's own serialised object: how it was read before `fast` existed.
    Plain,
    /// Both ways on every reading, the plain one used, and any difference reported at the end. For checking `fast`.
    Verify,
}

impl From<PageReadArg> for PageRead {
    fn from(arg: PageReadArg) -> Self {
        match arg {
            PageReadArg::Fast => Self::Fast,
            PageReadArg::Plain => Self::Plain,
            PageReadArg::Verify => Self::Verify,
        }
    }
}

pub fn parse_viewport(text: &str) -> Result<Viewport, ParseError> {
    Viewport::parse(text)
}

pub fn parse_browser(text: &str) -> Result<BrowserTarget, ParseError> {
    BrowserTarget::parse(text)
}
