//! How alike two names are. Our own code, so the rule is fixed and tested: a change of crate
//! would change which renames are proposed.

use unicode_normalization::UnicodeNormalization;

/// The least [`name_similarity`] that counts as evidence of a rename.
pub const NAME_THRESHOLD: f64 = 0.8;

/// Unicode NFC, lower case, whitespace trimmed and collapsed to single spaces.
pub fn normalise(text: &str) -> String {
    let composed: String = text.nfc().collect();
    composed.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The number of single-character insertions, deletions and substitutions that turn `a` into `b`
/// (characters are Unicode scalar values).
pub fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != cb)).min(above + 1).min(row[j] + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// 1 minus the edit distance over the length of the longer name, after [`normalise`]: 1.0 for names
/// that differ only in case, spacing or Unicode form, 0.0 for names with nothing in common.
///
/// Two empty names give 0.0: an unnamed control says nothing about which control it became, so
/// no evidence comes from it.
pub fn name_similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = normalise(a).chars().collect();
    let b: Vec<char> = normalise(b).chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }
    1.0 - levenshtein(&a, &b) as f64 / longest as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_and_spacing_do_not_matter() {
        assert!((name_similarity("Add  to Cart", " add to cart ") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn composed_and_decomposed_accents_are_equal() {
        assert!((name_similarity("caf\u{e9}", "cafe\u{301}") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_missing_accent_is_one_edit() {
        let s = name_similarity("caf\u{e9}", "cafe");
        assert!((s - 0.75).abs() < 1e-9, "{s}");
    }

    #[test]
    fn right_to_left_text_is_compared_by_character() {
        let s = name_similarity(
            "\u{627}\u{644}\u{62f}\u{641}\u{639}",
            "\u{627}\u{644}\u{62f}\u{641}\u{639}\u{629}",
        );
        assert!((s - 5.0 / 6.0).abs() < 1e-9, "{s}");
        assert!(name_similarity("\u{627}\u{628}", "\u{62c}\u{62f}") < 0.1);
    }

    #[test]
    fn empty_names_give_no_evidence() {
        assert!(name_similarity("", "").abs() < f64::EPSILON);
        assert!(name_similarity("", "Buy").abs() < f64::EPSILON);
        assert!(name_similarity("   ", "").abs() < f64::EPSILON);
    }

    #[test]
    fn one_edit_in_five_is_at_the_threshold() {
        assert!(name_similarity("Order", "Orders") >= NAME_THRESHOLD);
        assert!(name_similarity("Buy", "Sell") < NAME_THRESHOLD);
    }

    #[test]
    fn levenshtein_known_values() {
        let d = |a: &str, b: &str| levenshtein(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>());
        assert_eq!(d("kitten", "sitting"), 3);
        assert_eq!(d("", "abc"), 3);
        assert_eq!(d("abc", ""), 3);
        assert_eq!(d("same", "same"), 0);
    }
}
