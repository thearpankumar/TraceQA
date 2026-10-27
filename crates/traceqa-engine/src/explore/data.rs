//! Values for the fields the walker types into. No model: the value depends on the seed and the component only, so a
//! walk can be repeated. EXP-6 puts a language model in front of this and keeps it as the fallback.

use sha2::{Digest, Sha256};

/// What to type into the text field `component` (its id) with this accessible `name`.
pub fn value_for(seed: u64, component: &str, name: &str) -> String {
    let lower = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if has(&["email", "e-mail", "mail"]) {
        return "user@example.test".to_string();
    }
    if has(&["phone", "tel", "mobile"]) {
        return "5550100".to_string();
    }
    if has(&["zip", "postal", "pin code", "pincode"]) {
        return "12345".to_string();
    }
    if has(&["search", "find", "query", "look"]) {
        return "test".to_string();
    }
    let mut hash = Sha256::new();
    hash.update(seed.to_le_bytes());
    hash.update(component.as_bytes());
    let digest = hash.finalize();
    let number = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1000;
    format!("Test input {number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_and_component_give_the_same_value() {
        assert_eq!(value_for(1, "c_a", "Note"), value_for(1, "c_a", "Note"));
    }

    #[test]
    fn another_seed_or_component_usually_gives_another_value() {
        let values: std::collections::BTreeSet<String> = (0..20).map(|seed| value_for(seed, "c_a", "Note")).collect();
        assert!(values.len() > 10, "{values:?}");
        assert_ne!(value_for(1, "c_a", "Note"), value_for(1, "c_b", "Note"));
    }

    #[test]
    fn the_number_in_a_generated_value_is_below_a_thousand() {
        for seed in 0..50 {
            let value = value_for(seed, "c_x", "Note");
            let number: u32 = value.strip_prefix("Test input ").unwrap().parse().unwrap();
            assert!(number < 1000, "{value}");
        }
    }

    #[test]
    fn names_that_say_what_is_wanted_get_it() {
        assert_eq!(value_for(0, "c", "Your E-mail address"), "user@example.test");
        assert_eq!(value_for(0, "c", "Phone number"), "5550100");
        assert_eq!(value_for(0, "c", "ZIP code"), "12345");
        assert_eq!(value_for(0, "c", "Search the shop"), "test");
    }
}
