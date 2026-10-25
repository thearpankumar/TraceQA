//! Compares two `components.json` files by component id.
//!
//! [`compare`] and its building blocks are pure: they read no files and start no browser, so the
//! command line, the MCP server and the features that build on a diff (gates, healing, promoting a
//! reference) all use the same code. The one module that touches the disk is [`baseline`], which
//! keeps a folder of saved runs and is the only code that writes it.
//!
//! - [`compare`] gives a [`DiffFile`] (`spec/diff.schema.json`).
//! - [`summarise`] and [`changed_fields`] are the per-component building blocks.
//! - [`pair`] finds the removed component that an added one replaces after a change of role or name.
//! - [`markdown`] renders a diff for a pull request comment or a CI step summary.
//! - [`baseline`] saves, promotes and loads baselines; [`baseline::apply_aliases`] joins renames.
//! - [`gate`] reads `--fail-on` and decides which entries trip it.
//! - [`github`] and [`sarif`] write workflow commands and SARIF 2.1.0 for a diff or for findings.

pub mod baseline;
mod compare;
pub mod gate;
pub mod github;
mod markdown;
pub mod pair;
pub mod sarif;
pub mod text;

pub use baseline::{BaselineError, BaselineStore};
pub use compare::{changed_fields, compare, flags, summarise, DiffError, Options, FIELDS, FORMAT_VERSION};
pub use gate::{Gate, GateError, GateToken};
pub use markdown::{render_markdown, MAX_ROWS};
pub use traceqa_common::types::{
    ComponentSummary, DiffEntry, DiffEntryKind, DiffFile, DiffSide, DiffSummary, PairLink,
};

#[cfg(test)]
mod tests;
