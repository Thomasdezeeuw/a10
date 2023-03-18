#![feature(async_iterator, once_cell)]

use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::{Arc, Barrier};
use std::task::Poll;
use std::time::Instant;
use std::{env, fmt, io, panic, process, ptr, thread};

use a10::signals::{self, Signals};
use a10::Ring;

mod util;
use util::{is_send, is_sync, poll_nop, syscall};

const SIGNALS: [libc::c_int; 31] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGILL,
    libc::SIGTRAP,
    libc::SIGABRT,
    libc::SIGIOT,
    libc::SIGBUS,
    libc::SIGFPE,
    //libc::SIGKILL, // Can't handle this.
    libc::SIGUSR1,
    libc::SIGSEGV,
    libc::SIGUSR2,
    libc::SIGPIPE,
    libc::SIGALRM,
    libc::SIGTERM,
    libc::SIGSTKFLT,
    libc::SIGCHLD,
    libc::SIGCONT,
    //libc::SIGSTOP, // Can't handle this.
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
    libc::SIGURG,
    libc::SIGXCPU,
    libc::SIGXFSZ,
    libc::SIGVTALRM,
    libc::SIGPROF,
    libc::SIGWINCH,
    libc::SIGIO,
    libc::SIGPOLL, // NOTE: same value as `SIGIO`.
    libc::SIGPWR,
    libc::SIGSYS,
];

const SIGNAL_NAMES: [&str; SIGNALS.len()] = [
    "SIGHUP",
    "SIGINT",
    "SIGQUIT",
    "SIGILL",
    "SIGTRAP",
    "SIGABRT",
    "SIGIOT",
    "SIGBUS",
    "SIGFPE",
    //"SIGKILL",
    "SIGUSR1",
    "SIGSEGV",
    "SIGUSR2",
    "SIGPIPE",
    "SIGALRM",
    "SIGTERM",
    "SIGSTKFLT",
    "SIGCHLD",
    "SIGCONT",
    //"SIGSTOP",
    "SIGTSTP",
    "SIGTTIN",
    "SIGTTOU",
    "SIGURG",
    "SIGXCPU",
    "SIGXFSZ",
    "SIGVTALRM",
    "SIGPROF",
    "SIGWINCH",
    "SIGIO",
    "SIGPOLL",
    "SIGPWR",
    "SIGSYS",
];

fn main() {
    let start = Instant::now();
    println!("\nrunning {} tests", (2 * SIGNALS.len()) + 1);

    is_send::<Signals>();
    is_sync::<Signals>();
    is_send::<signals::Receive>();
    is_sync::<signals::Receive>();

    let quiet = env::args().any(|arg| matches!(&*arg, "-q" | "--quiet"));
    let mut harness = TestHarness::setup(quiet);
    harness.test_single_threaded();
    harness.test_multi_threaded();

    let mut passed = harness.passed;
    let mut failed = harness.failed;

    // Final test, make sure the cleanup is done properly.
    drop(harness);
    test_cleanup(quiet, &mut passed, &mut failed);

    println!("\ntest result: ok. {passed} passed; {failed} failed; 0 ignored; 0 measured; 0 filtered out; finished in {:?}\n", start.elapsed());
}

struct TestHarness {
    ring: Ring,
    signals: Signals,
    passed: usize,
    failed: usize,
    quiet: bool,
}

impl TestHarness {
    fn setup(quiet: bool) -> TestHarness {
        let ring = Ring::new(2).unwrap();
        let sq = ring.submission_queue().clone();
        let signals = Signals::from_signals(sq, SIGNALS).unwrap();
        TestHarness {
            ring,
            signals,
            passed: 0,
            failed: 0,
            quiet,
        }
    }

    fn test_single_threaded(&mut self) {
        let pid = process::id();
        for (signal, name) in SIGNALS.into_iter().zip(SIGNAL_NAMES) {
            print_test_start(self.quiet, format_args!("single_threaded ({name})"));
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                send_signal(pid, signal).unwrap();
                receive_signal(&mut self.ring, &self.signals, signal);
            }));
            if res.is_ok() {
                print_test_ok(self.quiet);
                self.passed += 1
            } else {
                print_test_failed(self.quiet);
                self.failed += 1
            };
        }
    }

    fn test_multi_threaded(&mut self) {
        // The main goals it ensure that the spawned thread isn't killed due to
        // the process signals.

        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        let handle = thread::spawn(move || {
            let pid = process::id();
            for signal in SIGNALS {
                send_signal(pid, signal).unwrap();

                // Linux doesn't guarantee the ordering of receiving signals,
                // but we do check for it. So, wait until the above signals is
                // received before sending the next one.
                b.wait();
            }
        });

        for (signal, name) in SIGNALS.into_iter().zip(SIGNAL_NAMES) {
            print_test_start(self.quiet, format_args!("multi_threaded ({name})"));
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                receive_signal(&mut self.ring, &self.signals, signal);
            }));
            if res.is_ok() {
                print_test_ok(self.quiet);
                self.passed += 1
            } else {
                print_test_failed(self.quiet);
                self.failed += 1
            };
            barrier.wait();
        }

        handle.join().unwrap();
    }
}

fn test_cleanup(quiet: bool, passed: &mut usize, failed: &mut usize) {
    print_test_start(quiet, format_args!("cleanup"));
    let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        // After `Signals` is dropped all signals should be unblocked.
        let set = blocked_signalset().unwrap();
        for signal in SIGNALS.into_iter() {
            assert!(!in_signalset(&set, signal));
        }
    }));
    if res.is_ok() {
        print_test_ok(quiet);
        *passed += 1
    } else {
        print_test_failed(quiet);
        *failed += 1
    };
}

fn receive_signal(ring: &mut Ring, signals: &Signals, expected_signal: libc::c_int) {
    let mut receive = signals.receive();
    let signal_info = loop {
        match poll_nop(Pin::new(&mut receive)) {
            Poll::Ready(result) => break result.unwrap(),
            Poll::Pending => ring.poll(None).unwrap(),
        }
    };
    assert_eq!(signal_info.ssi_signo as libc::c_int, expected_signal);
}

fn send_signal(pid: u32, signal: libc::c_int) -> std::io::Result<()> {
    syscall!(kill(pid as libc::pid_t, signal))?;
    Ok(())
}

fn blocked_signalset() -> io::Result<libc::sigset_t> {
    let mut old_set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    syscall!(sigprocmask(0, ptr::null_mut(), old_set.as_mut_ptr()))?;
    // SAFETY: `sigprocmask` initialised the signals set for us.
    Ok(unsafe { old_set.assume_init() })
}

fn in_signalset(set: &libc::sigset_t, signal: libc::c_int) -> bool {
    // SAFETY: we ensure the signal set is a valid pointer.
    unsafe { libc::sigismember(set, signal) == 1 }
}

fn print_test_start(quiet: bool, name: fmt::Arguments<'_>) {
    if !quiet {
        print!("test {name} ... ");
    }
}

fn print_test_ok(quiet: bool) {
    if quiet {
        print!(".")
    } else {
        print!("ok\n")
    }
}

fn print_test_failed(quiet: bool) {
    if quiet {
        print!("F")
    } else {
        print!("FAILED\n")
    }
}
