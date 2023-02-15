#![feature(async_iterator, once_cell)]

use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::{Arc, Barrier};
use std::task::Poll;
use std::time::Instant;
use std::{io, panic, process, ptr, thread};

use a10::signals::Signals;
use a10::Ring;

mod util;
use util::{poll_nop, syscall};

const SIGNALS: [libc::c_int; 5] = [
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGUSR1,
    libc::SIGUSR2,
];

const SIGNAL_NAMES: [&str; SIGNALS.len()] = ["SIGINT", "SIGQUIT", "SIGTERM", "SIGUSR1", "SIGUSR2"];

fn main() {
    let start = Instant::now();
    println!("\nrunning {} tests", (2 * SIGNALS.len()) + 1);

    let mut harness = TestHarness::setup();
    harness.test_single_threaded();
    harness.test_multi_threaded();

    let mut passed = harness.passed;
    let mut failed = harness.failed;

    // Final test, make sure the cleanup is done properly.
    drop(harness);
    test_cleanup(&mut passed, &mut failed);

    println!("\ntest result: ok. {passed} passed; {failed} failed; 0 ignored; 0 measured; 0 filtered out; finished in {:?}\n", start.elapsed());
}

struct TestHarness {
    ring: Ring,
    signals: Signals,
    passed: usize,
    failed: usize,
}

impl TestHarness {
    fn setup() -> TestHarness {
        let ring = Ring::new(2).unwrap();
        let sq = ring.submission_queue().clone();
        let signals = create_sigset().unwrap();
        let signals = Signals::new(sq, signals).unwrap();
        TestHarness {
            ring,
            signals,
            passed: 0,
            failed: 0,
        }
    }

    fn test_single_threaded(&mut self) {
        let pid = process::id();
        for (signal, name) in SIGNALS.iter().zip(SIGNAL_NAMES) {
            print!("test single_threaded ({name}) ... ");
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                send_signal(pid, *signal).unwrap();
                receive_signal(&mut self.ring, &self.signals, *signal);
            }));
            if res.is_ok() {
                print!("ok\n");
                self.passed += 1
            } else {
                print!("FAILED\n");
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

        for (signal, name) in SIGNALS.iter().zip(SIGNAL_NAMES) {
            print!("test multi_threaded ({name}) ... ");
            let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                receive_signal(&mut self.ring, &self.signals, *signal);
            }));
            if res.is_ok() {
                print!("ok\n");
                self.passed += 1
            } else {
                print!("FAILED\n");
                self.failed += 1
            };
            barrier.wait();
        }

        handle.join().unwrap();
    }
}

fn test_cleanup(passed: &mut usize, failed: &mut usize) {
    print!("test cleanup ... ");
    let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        // After `Signals` is dropped all signals should be unblocked.
        let set = blocked_signalset().unwrap();
        for signal in SIGNALS {
            assert!(!in_signalset(&set, signal));
        }
    }));
    if res.is_ok() {
        print!("ok\n");
        *passed += 1
    } else {
        print!("FAILED\n");
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

fn create_sigset() -> io::Result<libc::sigset_t> {
    let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    syscall!(sigemptyset(set.as_mut_ptr()))?;
    // SAFETY: initialised the set in the call to `sigemptyset`.
    let mut set = unsafe { set.assume_init() };
    for signal in SIGNALS {
        syscall!(sigaddset(&mut set, signal))?;
    }
    Ok(set)
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
    match syscall!(sigismember(set, signal)).unwrap() {
        1 => true,
        _ => false,
    }
}
