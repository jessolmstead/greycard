//! A sidecar of many tiny states, read: what it costs, counted by an
//! allocator of this binary's own, so the bound holds by bytes and not
//! by a clock. Its own test binary, one test, so nothing else
//! allocates beside the read.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use greycard_edit::{Edit, HISTORY, Sidecar};

/// The system's allocator, counting what is live and its peak.
struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is the system allocator's, the counts beside it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, handed on.
        let p = unsafe { System.alloc(l) };
        let now = LIVE.fetch_add(l.size(), Relaxed) + l.size();
        PEAK.fetch_max(now, Relaxed);
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: the caller's contract, handed on.
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// A line of `n` history states, each `{}` (three bytes a state on
/// disk, a whole default edit once read) but the first and the last,
/// at an exposure of 1 and 2 to tell them apart.
fn tiny_states(n: usize) -> String {
    let mut s = String::from(r#"{"current":{},"history":["#);
    for i in 0..n {
        s.push_str(match i {
            0 => r#"{"light":{"exposure":1.0}}"#,
            _ if i + 1 == n => r#",{"light":{"exposure":2.0}}"#,
            _ => ",{}",
        });
    }
    s.push_str("]}");
    s
}

/// 100,000 `{}` states (300 KB) took some 200 MB at their peak, each
/// read as a whole edit and given an id, before the read held a
/// history to the cap every build writes under. Now they cost what a
/// capped history does.
#[test]
fn a_file_of_many_tiny_states_reads_at_the_cost_of_a_capped_one() {
    let dir =
        std::env::temp_dir().join(format!("greycard-edit-many-states-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("frame.CR3.gcd");
    std::fs::write(&path, tiny_states(100_000)).unwrap();

    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let sidecar = Sidecar::read(&path, &Edit::default()).unwrap();
    let peak = PEAK.load(Relaxed) - base;
    greycard_core::testing::remove_dir_retry(&dir);

    // The cap's own rule: the first state and the newest after it.
    assert_eq!(sidecar.history.len(), HISTORY);
    let exposure = |i: usize| sidecar.history[i].edit().light.exposure;
    assert_eq!(
        (exposure(0), exposure(1), exposure(HISTORY - 1)),
        (1.0, 0.0, 2.0)
    );
    // The text and its parse are some 8 MB of the peak; every state
    // read whole was some 2 KB more.
    assert!(
        peak < 32 << 20,
        "a read of 100,000 tiny states peaked at {} MB",
        peak >> 20
    );
}
