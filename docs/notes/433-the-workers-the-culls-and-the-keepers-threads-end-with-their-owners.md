# §433. The worker's, the cull's, and the keeper's threads end with their owners

*2026-10-26*

CI on macOS failed the last six tests of greycard-ui with `WouldBlock`
spawning the worker and a culling thread. macOS caps a process's threads
far below Linux, and the suite never gave one back.

**What leaked.** Two things, each enough alone to leak. The worker's
loop and the culling prefetcher's waited on their queues with nothing to
say the owner was gone, as did the thread that makes an edit's pictures;
a dropped `Worker` or `Prefetcher` left them asleep. And in the tests
nothing was dropped at all: `testing::window` shows the App, a shown
window is held by the backend, and its callbacks hold the test's `State`
(which owns a prefetcher) and `Worker`. So every test with a window kept
a worker thread, its thumbnail pool, and up to three culling threads for
the rest of the run.

**What closes it.** `Worker`, `Prefetcher`, and the edit's keeper get a
`Drop` that marks the queue closed and wakes the threads; each ends
after the job in hand, and what was queued behind it is dropped, since
nobody is left to take it. The drop does not join: it runs on the
window's thread, and a develop or a decode can take seconds. The
worker's drop also stops its thumbnail pool, which the worker's thread
held alive. The test harness hides every window a test's thread showed
as that thread ends, so the App, its callbacks, the state, and the
worker drop with the test.

Left alone: the fetch and mosaic threads (one job each), the sidecar
read (not built in tests, one job), the indexer (ends on `Leave` or when
its channel closes) and its lane (ends with its channel), and the
reduced develops' pool (a rayon pool, dropped with the reducer).

**The counts.** greycard-ui's suite, 4 test threads, read from
`/proc/<pid>/status` a second before the process exits:

| | threads at start | at the end | peak |
|---|---|---|---|
| before | 1 | 5064 | 5265 |
| the drops alone | 1 | 4950 | 5151 |
| the drops and the harness | 1 | 72 | 273 |

What is left at the end is lavapipe's 32 threads and the render tests'
own, still running. A test drops a worker and a prefetcher and waits up
to two seconds for each queue to go: every thread holds its own queue,
so the check is per instance and the tests beside it cannot move it.
