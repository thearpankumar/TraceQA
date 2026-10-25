//! Keeping a panic on one thread from taking the work of the others with it.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Runs `work` and turns a panic into its message. This is a supervisor boundary, not error
/// handling: use it where a thread would otherwise die unseen, so the caller can still say that
/// the work failed. Only unwinding panics are caught; the panic hook still prints to stderr.
///
/// The closure is not required to be unwind safe, so it must reach shared state only through
/// locks that tolerate poisoning.
pub fn guard<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).map_err(|payload| panic_message(payload.as_ref()))
}

/// The text of a panic payload: what `panic!` was given, or "unknown panic".
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_returns_the_value_when_nothing_panics() {
        assert_eq!(guard(|| 7), Ok(7));
    }

    #[test]
    fn guard_returns_the_panic_message_for_str_and_string_payloads() {
        assert_eq!(guard(|| -> () { panic!("plain") }), Err("plain".to_string()));
        let n = 3;
        assert_eq!(
            guard(|| -> () { panic!("formatted {n}") }),
            Err("formatted 3".to_string())
        );
        assert_eq!(
            guard(|| -> () { std::panic::panic_any(42_u8) }),
            Err("unknown panic".to_string())
        );
    }
}
