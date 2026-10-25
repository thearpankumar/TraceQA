//! Compares two `components.json` files by component id.
//!
//! Pure: it reads no files and starts no browser, so the command line, the MCP server and the
//! features that build on a diff (gates, healing, promoting a reference) all use the same code.
//!
//! - [`compare`] gives a [`DiffFile`] (`spec/diff.schema.json`).
//! - [`summarise`] and [`changed_fields`] are the per-component building blocks.
//! - [`pair`] finds the removed component that an added one replaces after a change of role or name.
//! - [`markdown`] renders a diff for a pull request comment or a CI step summary.

mod compare;
mod markdown;
pub mod pair;
pub mod text;

pub use compare::{changed_fields, compare, flags, summarise, DiffError, Options, FIELDS, FORMAT_VERSION};
pub use markdown::{render_markdown, MAX_ROWS};
pub use traceqa_protocol::types::{
    ComponentSummary, DiffEntry, DiffEntryKind, DiffFile, DiffSide, DiffSummary, PairLink,
};

#[cfg(test)]
mod tests;
