#![cfg_attr(feature = "nightly", feature(async_iterator, cfg_sanitize))]

use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::{Arc, Barrier};
use std::task::Poll;
use std::time::Instant;
use std::{env, fmt, io, panic, process, ptr, task, thread};

use a10::fd::{Descriptor, Direct, File};
use a10::process::{ReceiveSignal, ReceiveSignals, Signals};
use a10::Ring;

mod util;
use util::{is_send, is_sync, poll_nop, syscall, NOP_WAKER};

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
    println!("\nrunning {} tests", (2 * (3 * SIGNALS.len()) + 1));

    is_send::<Signals<File>>();
    is_sync::<Signals<File>>();
    is_send::<Signals<Direct>>();
    is_sync::<Signals<Direct>>();
    is_send::<ReceiveSignal<File>>();
    is_sync::<ReceiveSignal<File>>();
    is_send::<ReceiveSignal<Direct>>();
    is_sync::<ReceiveSignal<Direct>>();
    is_send::<ReceiveSignals<File>>();
    is_sync::<ReceiveSignals<File>>();
    is_send::<ReceiveSignals<Direct>>();
    is_sync::<ReceiveSignals<Direct>>();

    let quiet = env::args().any(|arg| matches!(&*arg, "-q" | "--quiet"));
    let mut harness = TestHarness::setup(quiet);
    harness.test_single_threaded();
    harness.test_multi_threaded();
    harness.test_receive_signals();

    // Switch to use a direct descriptor.
    #[rustfmt::skip]
    let TestHarness { mut ring, signals, passed, failed, quiet } = harness;
    let signals = Some(to_direct(&mut ring, signals.unwrap()));
    #[rustfmt::skip]
    let mut harness = TestHarness { ring, signals, passed, failed, quiet };
    // Run the tests again.
    harness.test_single_threaded();
    harness.test_multi_threaded();
    harness.test_receive_signals();

    let mut passed = harness.passed;
    let mut failed = harness.failed;

    // Final test, make sure the cleanup is done properly.
    drop(harness);
    test_cleanup(quiet, &mut passed, &mut failed);

    println!("\ntest result: ok. {passed} passed; {failed} failed; 0 ignored; 0 measured; 0 filtered out; finished in {:.2?}s\n", start.elapsed().as_secs_f64());
}

struct TestHarness<D: Descriptor> {
    ring: Ring,
    signals: Option<Signals<D>>,
    passed: usize,
    failed: usize,
    quiet: bool,
}

impl TestHarness<File> {
    fn setup(quiet: bool) -> TestHarness<File> {
        let ring = Ring::config(2).with_direct_descriptors(2).build().unwrap();
        let sq = ring.submission_queue().clone();
        TestHarness {
            ring,
            signals: Some(Signals::from_signals(sq, SIGNALS).unwrap()),
            passed: 0,
            failed: 0,
            quiet,
        }
    }
}

impl<D: Descriptor> TestHarness<D> {
    fn test_single_threaded(&mut self) {
        let pid = process::id();
        let signals = self.signals.as_ref().unwrap();
        for (signal, name) in SIGNALS.into_iter().zip(SIGNAL_NAMES) {
            // thread sanitizer can't deal with `SIGSYS` signal being send.
            #[cfg(feature = "nightly")]
            if signal == libc::SIGSYS && cfg!(sanitize = "thread") {
                print_test_start(self.quiet, format_args!("single_threaded ({name})"));
                print_test_ignored(self.quiet);
                continue;
            }

            print_test_start(self.quiet, format_args!("single_threaded ({name})"));
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                send_signal(pid, signal).unwrap();
                receive_signal(&mut self.ring, &signals, signal);
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
                // thread sanitizer can't deal with `SIGSYS` signal being send.
                #[cfg(feature = "nightly")]
                if signal == libc::SIGSYS && cfg!(sanitize = "thread") {
                    continue;
                }

                send_signal(pid, signal).unwrap();

                // Linux doesn't guarantee the ordering of receiving signals,
                // but we do check for it. So, wait until the above signals is
                // received before sending the next one.
                b.wait();
            }
        });

        let signals = self.signals.as_ref().unwrap();
        for (signal, name) in SIGNALS.into_iter().zip(SIGNAL_NAMES) {
            // thread sanitizer can't deal with `SIGSYS` signal being send.
            #[cfg(feature = "nightly")]
            if signal == libc::SIGSYS && cfg!(sanitize = "thread") {
                print_test_start(self.quiet, format_args!("multi_threaded ({name})"));
                print_test_ignored(self.quiet);
                continue;
            }

            print_test_start(self.quiet, format_args!("multi_threaded ({name})"));
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                receive_signal(&mut self.ring, signals, signal);
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

    fn test_receive_signals(&mut self) {
        let pid = process::id();
        let mut receive_signal = self.signals.take().unwrap().receive_signals();
        let task_waker = unsafe { task::Waker::from_raw(NOP_WAKER) };
        let mut task_ctx = task::Context::from_waker(&task_waker);
        for (signal, name) in SIGNALS.into_iter().zip(SIGNAL_NAMES) {
            // thread sanitizer can't deal with `SIGSYS` signal being send.
            #[cfg(feature = "nightly")]
            if signal == libc::SIGSYS && cfg!(sanitize = "thread") {
                print_test_start(self.quiet, format_args!("single_threaded ({name})"));
                print_test_ignored(self.quiet);
                continue;
            }

            print_test_start(self.quiet, format_args!("single_threaded ({name})"));
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                send_signal(pid, signal).unwrap();

                // Check if the signals can be received.
                let signal_info = loop {
                    match Pin::new(&mut receive_signal).poll_next(&mut task_ctx) {
                        Poll::Ready(result) => break result.unwrap().unwrap(),
                        Poll::Pending => self.ring.poll(None).unwrap(),
                    }
                };
                assert_eq!(signal_info.ssi_signo as libc::c_int, signal);
            }));
            if res.is_ok() {
                print_test_ok(self.quiet);
                self.passed += 1
            } else {
                print_test_failed(self.quiet);
                self.failed += 1
            };
        }

        self.signals = Some(receive_signal.into_inner());
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

fn receive_signal<D: Descriptor>(
    ring: &mut Ring,
    signals: &Signals<D>,
    expected_signal: libc::c_int,
) {
    let mut receive = signals.receive();
    let signal_info = loop {
        match poll_nop(Pin::new(&mut receive)) {
            Poll::Ready(result) => break result.unwrap(),
            Poll::Pending => ring.poll(None).unwrap(),
        }
    };
    assert_eq!(signal_info.ssi_signo as libc::c_int, expected_signal);
}

fn to_direct(ring: &mut Ring, signals: Signals<File>) -> Signals<Direct> {
    let mut to_direct = signals.to_direct_descriptor();
    loop {
        match poll_nop(Pin::new(&mut to_direct)) {
            Poll::Ready(result) => break result.unwrap(),
            Poll::Pending => ring.poll(None).unwrap(),
        }
    }
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

#[cfg(feature = "nightly")]
fn print_test_ignored(quiet: bool) {
    if quiet {
        print!("i")
    } else {
        print!("ignored\n")
    }
}
