//! Ctrl-C, SIGTERM and SIGHUP for the commands that run for a while (`extract`, `login`, `serve`, `mcp`).
//!
//! The first signal sets [`CANCEL`]; the command then stops at its next step and closes what it
//! opened. A second signal, or a first one whose cleanup takes too long, ends the process at once
//! after killing the drivers it started. A first signal that does not finish by itself within
//! [`FIRST_SIGNAL_BOUND`] is treated as a second one, so `docker stop` (SIGKILL ten seconds after
//! SIGTERM) finds the run folders closed. ctrlc runs one handler for all three signals and does not
//! say which arrived, so all of them end the command with 130.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use traceqa_engine::Abort;

/// Set by the first signal. The command then stops at the next step and closes its browsers.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Set by the second signal. The engine then writes the final run folder at once, without waiting
/// for the browsers, and the watchdog ends the process.
pub static ABORT: Abort = Abort::new();

/// Exit code of a command stopped by Ctrl-C, SIGTERM or SIGHUP, as shells report an interrupted command.
pub const INTERRUPTED: u8 = 130;

/// How long the process waits for the run folders to be closed after a second signal.
const ABORT_WAIT: Duration = Duration::from_secs(2);

/// How long after a first signal the command may take to stop before it quits as if a second
/// signal had come. With [`ABORT_WAIT`] it is 9 s, under the 10 s that `docker stop` allows
/// between SIGTERM and SIGKILL.
pub const FIRST_SIGNAL_BOUND: Duration = Duration::from_secs(7);

/// What a command wants from the handler.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Printed on the first signal.
    pub first_message: &'static str,
    /// Whether the watchdog waits for the engine to close the run folders before exiting.
    pub waits_for_run_folder: bool,
    /// A server whose main thread is blocked reading its input: the watchdog ends the process
    /// as soon as the first signal has been seen and no run is in flight any more
    /// ([`ABORT`]'s in-flight count), because nothing else will.
    pub exits_when_idle: bool,
    /// Run by the watchdog just before the process exits (removes a temporary file).
    pub on_exit: fn(),
    /// End the process this long after the first signal even without a second one.
    pub first_signal_bound: Option<Duration>,
}

/// Writes a line to standard error and ignores a failure. `eprintln!` panics when the write fails,
/// and after SIGHUP the terminal is gone.
pub fn say(text: &str) {
    let _ = writeln!(io::stderr(), "{text}");
}

/// What the watchdog does next.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    /// Leave now, nothing is left to close.
    ExitIdle,
    /// Close the run folders, kill the drivers and leave. `timed_out`: the first signal's
    /// cleanup took too long, so the abort is requested here and not by a second signal.
    Quit {
        timed_out: bool,
    },
}

/// The watchdog's decision, from what it sees: a second signal ends the process, a first one
/// ends a server that has nothing in flight, and a first one that has gone on for `bound` ends
/// it too.
fn next_step(
    cancelled: bool,
    abort_requested: bool,
    idle_server: bool,
    since_first: Option<Duration>,
    bound: Option<Duration>,
) -> Step {
    if abort_requested {
        Step::Quit { timed_out: false }
    } else if cancelled && idle_server {
        Step::ExitIdle
    } else if cancelled && matches!((since_first, bound), (Some(since), Some(bound)) if since >= bound) {
        Step::Quit { timed_out: true }
    } else {
        Step::Wait
    }
}

/// Installs the handler and the watchdog thread. Call once per process (ctrlc allows one handler).
pub fn install(hooks: Hooks) {
    let first_message = hooks.first_message;
    // The handler only sets flags, so no file or process work happens inside it.
    let handler = move || {
        if CANCEL.swap(true, Ordering::SeqCst) {
            ABORT.request();
            return;
        }
        say(first_message);
    };
    if let Err(error) = ctrlc::set_handler(handler) {
        say(&format!(
            "traceqa: Ctrl-C will stop the command without closing it cleanly: {error}"
        ));
        return;
    }
    let watchdog = std::thread::Builder::new()
        .name("abort-watchdog".into())
        .spawn(move || {
            let mut first_seen: Option<Instant> = None;
            loop {
                let cancelled = CANCEL.load(Ordering::SeqCst);
                if cancelled {
                    first_seen.get_or_insert_with(Instant::now);
                }
                let idle_server = cancelled && hooks.exits_when_idle && ABORT.wait_idle(Duration::ZERO);
                match next_step(
                    cancelled,
                    ABORT.is_requested(),
                    idle_server,
                    first_seen.map(|since| since.elapsed()),
                    hooks.first_signal_bound,
                ) {
                    Step::Wait => {}
                    Step::ExitIdle => break,
                    Step::Quit { timed_out } => {
                        if timed_out {
                            say("\nStopping took too long; quitting.");
                            // As a second signal would: the engine writes the run folders now.
                            ABORT.request();
                        } else {
                            say("\nQuitting at once.");
                        }
                        if hooks.waits_for_run_folder {
                            if hooks.exits_when_idle {
                                ABORT.wait_quiet(ABORT_WAIT);
                            } else {
                                ABORT.wait_closed(ABORT_WAIT);
                            }
                        }
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            traceqa_engine::driver::kill_all_drivers();
            (hooks.on_exit)();
            std::process::exit(i32::from(INTERRUPTED));
        });
    if let Err(error) = watchdog {
        say(&format!("traceqa: a second Ctrl-C will not quit at once: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: Option<Duration> = Some(Duration::from_secs(7));

    fn secs(n: u64) -> Option<Duration> {
        Some(Duration::from_secs(n))
    }

    #[test]
    fn nothing_happens_without_a_signal() {
        assert_eq!(next_step(false, false, false, None, BOUND), Step::Wait);
    }

    #[test]
    fn a_second_signal_quits_at_once() {
        assert_eq!(
            next_step(true, true, false, secs(1), BOUND),
            Step::Quit { timed_out: false }
        );
    }

    #[test]
    fn a_first_signal_waits_until_the_bound_and_then_escalates() {
        assert_eq!(next_step(true, false, false, secs(6), BOUND), Step::Wait);
        assert_eq!(
            next_step(true, false, false, secs(7), BOUND),
            Step::Quit { timed_out: true }
        );
        assert_eq!(
            next_step(true, false, false, secs(60), BOUND),
            Step::Quit { timed_out: true }
        );
    }

    #[test]
    fn a_first_signal_without_a_bound_waits_for_ever() {
        assert_eq!(next_step(true, false, false, secs(3600), None), Step::Wait);
    }

    #[test]
    fn a_server_with_nothing_in_flight_leaves_on_the_first_signal() {
        assert_eq!(next_step(true, false, true, secs(0), BOUND), Step::ExitIdle);
        // But never before a signal arrived.
        assert_eq!(next_step(false, false, false, None, BOUND), Step::Wait);
    }

    #[test]
    fn the_bound_and_the_abort_wait_fit_inside_dockers_grace_period() {
        assert!(FIRST_SIGNAL_BOUND + ABORT_WAIT < Duration::from_secs(10));
    }
}
