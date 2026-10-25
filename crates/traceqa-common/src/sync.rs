//! One policy for poisoned locks.
//!
//! A lock is poisoned when a thread panicked while holding it. Everything in TraceQA that is
//! guarded by a lock stays usable after that: a run that panicked is reported as failed (see
//! [`supervise`](crate::supervise)), and the server keeps answering. So a poisoned lock is
//! recovered, and this is the one place that says so.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Locking that ignores poisoning.
pub trait LockExt<T> {
    /// The guard, also when another thread panicked while holding the lock.
    fn lock_ok(&self) -> MutexGuard<'_, T>;

    /// The value, also when another thread panicked while holding the lock.
    fn into_inner_ok(self) -> T
    where
        Self: Sized;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_ok(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn into_inner_ok(self) -> T {
        self.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Waiting on a condition variable, ignoring poisoning.
pub trait CondvarExt {
    /// As [`Condvar::wait_timeout`], returning only the guard. The caller checks its predicate
    /// again, as after any wake-up.
    fn wait_timeout_ok<'a, T>(&self, guard: MutexGuard<'a, T>, timeout: Duration) -> MutexGuard<'a, T>;
}

impl CondvarExt for Condvar {
    fn wait_timeout_ok<'a, T>(&self, guard: MutexGuard<'a, T>, timeout: Duration) -> MutexGuard<'a, T> {
        self.wait_timeout(guard, timeout)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn lock_ext_recovers_a_poisoned_mutex() {
        let lock = Arc::new(Mutex::new(5));
        let poisoner = Arc::clone(&lock);
        let _ = thread::spawn(move || {
            let mut guard = poisoner.lock().unwrap();
            *guard = 6;
            panic!("poisons the lock");
        })
        .join();
        assert!(lock.lock().is_err(), "the lock is poisoned");
        assert_eq!(*lock.lock_ok(), 6);
        let lock = Arc::try_unwrap(lock).unwrap();
        assert_eq!(lock.into_inner_ok(), 6);
    }

    #[test]
    fn a_wait_on_a_poisoned_mutex_still_returns() {
        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        let poisoner = Arc::clone(&pair);
        let _ = thread::spawn(move || {
            let _guard = poisoner.0.lock().unwrap();
            panic!("poisons the lock");
        })
        .join();
        let guard = pair.0.lock_ok();
        let guard = pair.1.wait_timeout_ok(guard, Duration::from_millis(10));
        assert!(!*guard);
    }
}
