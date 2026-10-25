//! Pairing a removed component with the added one that is probably the same component.
//!
//! A component id is a hash of version, role, normalised name and path (`spec/fingerprint.md`),
//! so a button that becomes a link, or is renamed, or turns into a template, is a different id
//! on the other side. The diff therefore reports it as one removed and one added entry. This
//! pass finds such couples. It is public because other features that follow a component across
//! a change (healing a locator, aliases for a promoted reference) need the same answer.

use std::collections::BTreeMap;

use traceqa_protocol::types::Confidence;

use crate::ComponentSummary;

/// Same path and name, other role.
pub const ROLE_CHANGED: &str = "role_changed";
/// Same path and role, other name or template flag.
pub const NAME_CHANGED: &str = "name_changed";

/// One side of a possible pair.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub id: &'a str,
    pub summary: &'a ComponentSummary,
}

/// A removed and an added component that look like the same one. The indexes point into the
/// slices given to [`pair`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub removed: usize,
    pub added: usize,
    /// [`ROLE_CHANGED`] or [`NAME_CHANGED`].
    pub reason: &'static str,
    pub confidence: Confidence,
}

/// Pairs removed with added components. Only components with the same path are compared.
///
/// 1. Same name and template flag, other role: paired when each has exactly one such partner
///    (reason `role_changed`, confidence high).
/// 2. Of what is left, same role: paired when the path has exactly one removed and one added
///    component of that role (reason `name_changed`, confidence low). Two renames at one path
///    are ambiguous and pair nothing.
///
/// Every index is in at most one pair. The result is ordered by the removed index.
pub fn pair(removed: &[Candidate<'_>], added: &[Candidate<'_>]) -> Vec<Pair> {
    let mut buckets: BTreeMap<&str, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (i, c) in removed.iter().enumerate() {
        buckets.entry(c.summary.path.as_str()).or_default().0.push(i);
    }
    for (i, c) in added.iter().enumerate() {
        buckets.entry(c.summary.path.as_str()).or_default().1.push(i);
    }
    let mut pairs = Vec::new();
    for (gone, new) in buckets.values() {
        pair_bucket(removed, added, gone, new, &mut pairs);
    }
    pairs.sort_by_key(|p| p.removed);
    pairs
}

fn pair_bucket(removed: &[Candidate<'_>], added: &[Candidate<'_>], gone: &[usize], new: &[usize], out: &mut Vec<Pair>) {
    let mut gone: Vec<usize> = gone.to_vec();
    let mut new: Vec<usize> = new.to_vec();

    let same_name = |r: usize, a: usize| {
        let (r, a) = (removed[r].summary, added[a].summary);
        r.name == a.name && r.name_is_template == a.name_is_template && r.role != a.role
    };
    let mut taken = Vec::new();
    for &r in &gone {
        let partners: Vec<usize> = new.iter().copied().filter(|&a| same_name(r, a)).collect();
        if let [a] = partners[..] {
            if gone.iter().filter(|&&other| same_name(other, a)).count() == 1 {
                taken.push((r, a, ROLE_CHANGED, Confidence::High));
            }
        }
    }
    remove_taken(&mut gone, &mut new, &taken, out);

    let mut taken = Vec::new();
    for &r in &gone {
        let role = &removed[r].summary.role;
        let same_role = |a: &&usize| &added[**a].summary.role == role;
        let partners: Vec<usize> = new.iter().filter(same_role).copied().collect();
        let others = gone.iter().filter(|&&o| &removed[o].summary.role == role).count();
        if let [a] = partners[..] {
            if others == 1 {
                taken.push((r, a, NAME_CHANGED, Confidence::Low));
            }
        }
    }
    remove_taken(&mut gone, &mut new, &taken, out);
}

fn remove_taken(
    gone: &mut Vec<usize>,
    new: &mut Vec<usize>,
    taken: &[(usize, usize, &'static str, Confidence)],
    out: &mut Vec<Pair>,
) {
    for &(removed, added, reason, confidence) in taken {
        gone.retain(|&i| i != removed);
        new.retain(|&i| i != added);
        out.push(Pair {
            removed,
            added,
            reason,
            confidence,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(role: &str, name: &str, template: bool, path: &str) -> ComponentSummary {
        ComponentSummary {
            role: role.into(),
            name: name.into(),
            name_is_template: template,
            path: path.into(),
            instances: 1,
            visible: 1,
            enabled: 1,
            covered: 0,
            checked: 0,
            expanded: 0,
            selected: 0,
            pressed: 0,
            confidence: Confidence::High,
        }
    }

    fn run(removed: &[ComponentSummary], added: &[ComponentSummary]) -> Vec<Pair> {
        let r: Vec<Candidate> = removed.iter().map(|s| Candidate { id: "r", summary: s }).collect();
        let a: Vec<Candidate> = added.iter().map(|s| Candidate { id: "a", summary: s }).collect();
        pair(&r, &a)
    }

    #[test]
    fn a_role_change_pairs_with_high_confidence() {
        let pairs = run(
            &[summary("button", "Buy", false, "main")],
            &[summary("link", "Buy", false, "main")],
        );
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].reason, pairs[0].confidence), (ROLE_CHANGED, Confidence::High));
    }

    #[test]
    fn a_unique_rename_pairs_with_low_confidence() {
        let pairs = run(
            &[summary("button", "Buy", false, "main")],
            &[summary("button", "Purchase", false, "main")],
        );
        assert_eq!((pairs[0].reason, pairs[0].confidence), (NAME_CHANGED, Confidence::Low));
    }

    #[test]
    fn a_template_flip_pairs_as_a_name_change() {
        let pairs = run(
            &[summary("button", "*", true, "main")],
            &[summary("button", "Buy", false, "main")],
        );
        assert_eq!(pairs[0].reason, NAME_CHANGED);
    }

    #[test]
    fn two_renames_at_one_path_pair_nothing() {
        let removed = [
            summary("button", "A", false, "main"),
            summary("button", "B", false, "main"),
        ];
        let added = [
            summary("button", "C", false, "main"),
            summary("button", "D", false, "main"),
        ];
        assert!(run(&removed, &added).is_empty());
    }

    #[test]
    fn another_path_never_pairs() {
        let pairs = run(
            &[summary("button", "Buy", false, "main")],
            &[summary("link", "Buy", false, "nav")],
        );
        assert!(pairs.is_empty());
    }

    #[test]
    fn an_ambiguous_role_change_is_left_alone() {
        let removed = [summary("button", "Buy", false, "main")];
        let added = [
            summary("link", "Buy", false, "main"),
            summary("menuitem", "Buy", false, "main"),
        ];
        assert!(run(&removed, &added).is_empty());
    }

    #[test]
    fn a_role_pair_leaves_the_rest_to_the_rename_rule() {
        let removed = [
            summary("button", "Buy", false, "main"),
            summary("link", "Old", false, "main"),
        ];
        let added = [
            summary("link", "Buy", false, "main"),
            summary("link", "New", false, "main"),
        ];
        // button "Buy" -> link "Buy" is a role change; then link "Old" -> link "New" is the
        // only link left on each side.
        let pairs = run(&removed, &added);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].reason, ROLE_CHANGED);
        assert_eq!(pairs[1].reason, NAME_CHANGED);
    }
}
