#![cfg_attr(feature = "nightly", feature(async_iterator, cfg_sanitize))]

use std::mem::MaybeUninit;
use std::sync::{Arc, Barrier};
use std::time::Instant;
use std::{env, fmt, io, panic, ptr, thread};

use a10::Ring;
use a10::fd;
use a10::process::{self, Signal, Signals, To};

mod util;
use util::{block_on, init, next, syscall};

const SIGNALS: &[Signal] = &[
    Signal::HUP,
    Signal::INTERRUPT,
    Signal::QUIT,
    Signal::ILLEGAL,
    Signal::TRAP,
    Signal::ABORT,
    Signal::IOT,
    Signal::BUS,
    Signal::FP_ERROR,
    Signal::USER1,
    Signal::USER2,
    Signal::SEG_VAULT,
    Signal::PIPE,
    Signal::ALARM,
    Signal::TERMINATION,
    Signal::CHILD,
    Signal::CONTINUE,
    Signal::TERM_STOP,
    Signal::TTY_IN,
    Signal::TTY_OUT,
    Signal::URGENT,
    Signal::EXCEEDED_CPU,
    Signal::EXCEEDED_FILE_SIZE,
    Signal::VIRTUAL_ALARM,
    Signal::PROFILE_ALARM,
    Signal::WINDOW_RESIZE,
    Signal::IO,
    #[cfg(any(target_os = "android", target_os = "linux"))]
    Signal::POLL, // NOTE: same value as `IO`.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    Signal::PWR,
    Signal::SYS,
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
    "SIGUSR1",
    "SIGUSR2",
    "SIGSEGV",
    "SIGPIPE",
    "SIGALRM",
    "SIGTERM",
    "SIGCHLD",
    "SIGCONT",
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
    #[cfg(any(target_os = "android", target_os = "linux"))]
    "SIGPOLL",
    #[cfg(any(target_os = "android", target_os = "linux"))]
    "SIGPWR",
    "SIGSYS",
];

fn main() {
    init();
    let start = Instant::now();
    println!("\nrunning {} tests", (2 * (3 * SIGNALS.len()) + 1));

    let quiet = env::args().any(|arg| matches!(&*arg, "-q" | "--quiet"));
    let mut harness = TestHarness::setup(quiet);
    harness.run_tests();

    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        // Switch to use a direct descriptor.
        harness.signals = Some(
            block_on(
                &mut harness.ring,
                harness.signals.take().unwrap().to_direct_descriptor(),
            )
            .unwrap(),
        );
        harness.fd_kind = fd::Kind::Direct;
        harness.run_tests();
    }

    // Final test, make sure the cleanup is done properly.
    drop(harness.signals.take());
    harness.test_cleanup();

    println!(
        "\ntest result: ok. {} passed; {} failed; 0 ignored; 0 measured; 0 filtered out; finished in {:.2?}s\n",
        harness.passed,
        harness.failed,
        start.elapsed().as_secs_f64(),
    );
}

struct TestHarness {
    ring: Ring,
    signals: Option<Signals>,
    fd_kind: fd::Kind,
    passed: usize,
    failed: usize,
    quiet: bool,
}

impl TestHarness {
    fn setup(quiet: bool) -> TestHarness {
        let config = Ring::config();
        #[cfg(any(target_os = "android", target_os = "linux"))]
        let config = config.with_direct_descriptors(2);
        let ring = config.build().unwrap();
        let sq = ring.sq();
        TestHarness {
            ring,
            signals: Some(Signals::for_all_signals(sq).unwrap()),
            fd_kind: fd::Kind::File,
            passed: 0,
            failed: 0,
            quiet,
        }
    }

    fn run_tests(&mut self) {
        self.test_single_threaded();
        self.test_multi_threaded();
        self.test_receive_signals();
    }

    fn run_test<F>(&mut self, test_name: &str, mut test: F)
    where
        F: FnMut(&mut Ring, &Signals, Signal),
    {
        let signals = self.signals.as_ref().unwrap();
        for (signal, name) in SIGNALS.iter().copied().zip(SIGNAL_NAMES) {
            print_test_start(
                self.quiet,
                format_args!("{test_name} ({:?}, {name})", self.fd_kind),
            );
            // thread sanitizer can't deal with `SIGSYS` signal being send.
            #[cfg(feature = "nightly")]
            if signal == Signal::SYS && cfg!(sanitize = "thread") {
                print_test_ignored(self.quiet);
                continue;
            }
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                test(&mut self.ring, signals, signal);
            }));
            print_test_result(result, self.quiet, &mut self.passed, &mut self.failed);
        }
    }

    fn test_single_threaded(&mut self) {
        let pid = std::process::id();
        self.run_test("single_threaded", |ring, signals, signal| {
            process::send_signal(To::Process(pid), signal).unwrap();
            receive_signal(ring, signals, signal);
        });
    }

    fn test_multi_threaded(&mut self) {
        // The main goals it ensure that the spawned thread isn't killed due to
        // the process signals.

        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        let handle = thread::spawn(move || {
            let pid = std::process::id();
            for signal in SIGNALS.iter().copied() {
                // thread sanitizer can't deal with `SIGSYS` signal being send.
                #[cfg(feature = "nightly")]
                if signal == Signal::SYS && cfg!(sanitize = "thread") {
                    continue;
                }

                process::send_signal(To::Process(pid), signal).unwrap();

                // Linux doesn't guarantee the ordering of receiving signals,
                // but we do check for it. So, wait until the above signals is
                // received before sending the next one.
                b.wait();
            }
        });

        self.run_test("multi_threaded", |ring, signals, signal| {
            receive_signal(ring, signals, signal);
            barrier.wait(); // Send the next signal.
        });

        handle.join().unwrap();
    }

    fn test_receive_signals(&mut self) {
        let pid = std::process::id();
        let mut receive_signal = self.signals.take().unwrap().receive_signals();
        for (signal, name) in SIGNALS.into_iter().zip(SIGNAL_NAMES) {
            print_test_start(
                self.quiet,
                format_args!("receive_signals ({:?}, {name})", self.fd_kind),
            );
            // thread sanitizer can't deal with `SIGSYS` signal being send.
            #[cfg(feature = "nightly")]
            if signal == Signal::SYS && cfg!(sanitize = "thread") {
                print_test_ignored(self.quiet);
                continue;
            }
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                process::send_signal(To::Process(pid), *signal).unwrap();

                // Check if the signals can be received.
                let signal_info = block_on(&mut self.ring, next(&mut receive_signal))
                    .unwrap()
                    .unwrap();
                assert_eq!(signal_info.signal(), *signal);
            }));
            print_test_result(result, self.quiet, &mut self.passed, &mut self.failed);
        }

        self.signals = Some(receive_signal.into_inner());
    }

    fn test_cleanup(&mut self) {
        print_test_start(self.quiet, format_args!("cleanup"));
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            // After `Signals` is dropped all signals should be unblocked.
            let set = blocked_signalset().unwrap();
            for signal in SIGNALS.iter().copied() {
                // SAFETY: this is not safe.
                let signo = unsafe { std::mem::transmute(signal) };
                assert!(!in_signalset(&set, signo));
            }
        }));
        print_test_result(result, self.quiet, &mut self.passed, &mut self.failed);
    }
}

fn receive_signal(ring: &mut Ring, signals: &Signals, expected_signal: Signal) {
    let signal_info = block_on(ring, signals.receive()).unwrap();
    assert_eq!(signal_info.signal(), expected_signal);
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

#[allow(clippy::needless_pass_by_value)]
fn print_test_result(
    result: thread::Result<()>,
    quiet: bool,
    passed: &mut usize,
    failed: &mut usize,
) {
    if result.is_ok() {
        print!("{}", if quiet { "." } else { "ok\n" });
        *passed += 1;
    } else {
        print!("{}", if quiet { "F" } else { "FAILED\n" });
        *failed += 1;
    }
}

#[cfg(feature = "nightly")]
fn print_test_ignored(quiet: bool) {
    print!("{}", if quiet { "i" } else { "ignored\n" });
}
