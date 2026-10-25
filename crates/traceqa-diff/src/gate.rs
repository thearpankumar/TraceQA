//! The `--fail-on` gate, and the one classification of a diff entry that the gate, the GitHub
//! annotations and the SARIF output share.
//!
//! An entry that an alias links to its counterpart (`pairedWith.reason == "aliased"`, see
//! `baseline::apply_aliases`) is a rename the person accepted: no gate token matches it.

use std::collections::BTreeSet;
use std::fmt;

use traceqa_common::types::{DiffEntry, DiffEntryKind, DiffFile};

/// `PairLink.reason` of a pair that a baseline alias made.
pub const ALIASED: &str = "aliased";

/// What `--fail-on` can name. An entry matches a token when it carries the flag of that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateToken {
    /// In the baseline only.
    Removed,
    /// In the run only.
    Added,
    /// On both sides with any difference.
    Changed,
    /// `became_disabled`.
    Disabled,
    /// `became_hidden`.
    Hidden,
    /// `became_covered`.
    Covered,
    /// A removed or added entry that looks like the other side's after a change of role or name.
    Paired,
}

impl GateToken {
    /// Every token, in the order they are listed in help and errors.
    pub const ALL: [GateToken; 7] = [
        Self::Removed,
        Self::Added,
        Self::Changed,
        Self::Disabled,
        Self::Hidden,
        Self::Covered,
        Self::Paired,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Disabled => "disabled",
            Self::Hidden => "hidden",
            Self::Covered => "covered",
            Self::Paired => "paired",
        }
    }

    fn matches(self, entry: &DiffEntry) -> bool {
        if is_aliased(entry) {
            return false;
        }
        let has = |flag: &str| entry.flags.iter().any(|f| f == flag);
        match self {
            Self::Removed => entry.kind == DiffEntryKind::Removed,
            Self::Added => entry.kind == DiffEntryKind::Added,
            Self::Changed => entry.kind == DiffEntryKind::Changed,
            Self::Disabled => has("became_disabled"),
            Self::Hidden => has("became_hidden"),
            Self::Covered => has("became_covered"),
            Self::Paired => entry.paired_with.is_some(),
        }
    }
}

/// Why a `--fail-on` value is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    Unknown(String),
    Empty,
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let known: Vec<&str> = GateToken::ALL.iter().map(|t| t.name()).collect();
        match self {
            Self::Unknown(token) => write!(
                f,
                "--fail-on: unknown token {token:?}; use a comma-separated list of {}",
                known.join(", ")
            ),
            Self::Empty => write!(f, "--fail-on: expected a comma-separated list of {}", known.join(", ")),
        }
    }
}

impl std::error::Error for GateError {}

/// A set of tokens. The empty gate matches nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gate {
    tokens: BTreeSet<GateToken>,
}

impl Gate {
    /// Parses `removed,disabled`. Case and spaces around a token are ignored; an unknown or empty
    /// token is an error, so a typo cannot turn the gate off.
    pub fn parse(text: &str) -> Result<Self, GateError> {
        let mut tokens = BTreeSet::new();
        for part in text.split(',') {
            let part = part.trim().to_ascii_lowercase();
            if part.is_empty() {
                return Err(GateError::Empty);
            }
            let token = GateToken::ALL
                .iter()
                .find(|t| t.name() == part)
                .ok_or_else(|| GateError::Unknown(part.clone()))?;
            tokens.insert(*token);
        }
        Ok(Self { tokens })
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The token names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.tokens.iter().map(|t| t.name().to_string()).collect()
    }

    /// Whether this entry trips the gate.
    pub fn matches(&self, entry: &DiffEntry) -> bool {
        self.tokens.iter().any(|t| t.matches(entry))
    }

    /// How many entries of the diffs trip the gate.
    pub fn count<'a>(&self, diffs: impl IntoIterator<Item = &'a DiffFile>) -> usize {
        diffs
            .into_iter()
            .flat_map(|d| &d.entries)
            .filter(|e| self.matches(e))
            .count()
    }
}

/// Whether an alias made this entry's pair.
pub fn is_aliased(entry: &DiffEntry) -> bool {
    entry.paired_with.as_ref().is_some_and(|p| p.reason == ALIASED)
}

/// How an entry is shown by the formats that need one rule per entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Renamed,
    Removed,
    Added,
    Disabled,
    Hidden,
    Covered,
    Changed,
}

/// How serious an entry is to a person reading annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Self::Renamed => "renamed",
            Self::Removed => "removed",
            Self::Added => "added",
            Self::Disabled => "disabled",
            Self::Hidden => "hidden",
            Self::Covered => "covered",
            Self::Changed => "changed",
        }
    }

    /// Lost or broken controls are errors; new or merely different ones are warnings.
    pub fn severity(self) -> Severity {
        match self {
            Self::Removed | Self::Disabled | Self::Hidden | Self::Covered => Severity::Error,
            Self::Added | Self::Changed => Severity::Warning,
            Self::Renamed => Severity::Note,
        }
    }
}

/// The class of an entry. A changed entry that became disabled, hidden or covered takes the
/// first of those, in that order.
pub fn classify(entry: &DiffEntry) -> Class {
    if is_aliased(entry) {
        return Class::Renamed;
    }
    match entry.kind {
        DiffEntryKind::Removed => Class::Removed,
        DiffEntryKind::Added => Class::Added,
        DiffEntryKind::Changed => {
            let has = |flag: &str| entry.flags.iter().any(|f| f == flag);
            if has("became_disabled") {
                Class::Disabled
            } else if has("became_hidden") {
                Class::Hidden
            } else if has("became_covered") {
                Class::Covered
            } else {
                Class::Changed
            }
        }
    }
}

/// The entries worth one line each: an aliased pair is one entry (the removed side), not two.
pub fn reportable(diff: &DiffFile) -> impl Iterator<Item = &DiffEntry> {
    diff.entries
        .iter()
        .filter(|e| !(is_aliased(e) && e.kind == DiffEntryKind::Added))
}

#[cfg(test)]
mod tests {
    use super::*;
    use traceqa_common::types::PairLink;

    fn entry(kind: DiffEntryKind, flags: &[&str]) -> DiffEntry {
        DiffEntry {
            kind,
            id: "c_000000000001".into(),
            a: None,
            b: None,
            fields: Vec::new(),
            flags: flags.iter().map(|f| (*f).to_string()).collect(),
            paired_with: None,
        }
    }

    #[test]
    fn tokens_parse_with_case_and_spaces() {
        let gate = Gate::parse("Removed, disabled ,HIDDEN").unwrap();
        assert_eq!(gate.names(), ["removed", "disabled", "hidden"]);
    }

    #[test]
    fn an_unknown_or_empty_token_is_an_error() {
        assert_eq!(Gate::parse("removd"), Err(GateError::Unknown("removd".into())));
        assert_eq!(Gate::parse("removed,,added"), Err(GateError::Empty));
        assert_eq!(Gate::parse(""), Err(GateError::Empty));
        assert!(Gate::parse("nope").unwrap_err().to_string().contains("paired"));
    }

    #[test]
    fn tokens_match_kinds_and_flags() {
        let gate = Gate::parse("removed,disabled").unwrap();
        assert!(gate.matches(&entry(DiffEntryKind::Removed, &[])));
        assert!(!gate.matches(&entry(DiffEntryKind::Added, &[])));
        assert!(gate.matches(&entry(DiffEntryKind::Changed, &["became_disabled", "count_up"])));
        assert!(!gate.matches(&entry(DiffEntryKind::Changed, &["became_hidden"])));
        assert!(Gate::parse("changed")
            .unwrap()
            .matches(&entry(DiffEntryKind::Changed, &[])));
    }

    #[test]
    fn an_aliased_pair_trips_nothing() {
        let all = Gate::parse("removed,added,changed,paired").unwrap();
        let mut removed = entry(DiffEntryKind::Removed, &[]);
        removed.paired_with = Some(PairLink {
            id: "c_000000000002".into(),
            reason: ALIASED.into(),
            confidence: traceqa_common::types::Confidence::High,
        });
        assert!(!all.matches(&removed));
        assert_eq!(classify(&removed), Class::Renamed);
        removed.paired_with.as_mut().unwrap().reason = "name_changed".into();
        assert!(all.matches(&removed));
    }

    #[test]
    fn classes_have_a_fixed_priority() {
        let both = entry(DiffEntryKind::Changed, &["became_hidden", "became_disabled"]);
        assert_eq!(classify(&both), Class::Disabled);
        assert_eq!(classify(&entry(DiffEntryKind::Changed, &["count_up"])), Class::Changed);
    }
}
