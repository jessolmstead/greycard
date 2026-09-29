# 201. Threads and channels, not async (2026-09-28)

The question came up whether the editor should move to async/await.
The answer for now is no, and this is the reasoning, so a later
revisit starts from it rather than from scratch.

**What the code waits on.** Almost everything is CPU: the demosaic,
the denoisers, the develop stages, the thumbnail decodes. Rayon and
plain threads are the right tool for that, and async gives nothing to
compute-bound work. The window has a scheduler already, Slint's event
loop, and the one pattern in use is: do the work on a thread of its
own or on the pool, land the result on the window's thread through
`invoke_from_event_loop`, and never hold the window while waiting.
§187 and §188 are that pattern applied to the network share; the
worker is one long-lived thread fed by a channel; the indexer is
another. The only `.await` in the tree is the three lines where rfd's
native file chooser is asynchronous by design, driven to completion in
place, and the GPU crates drive wgpu's futures with `pollster`.

**Where async looks tempting.** The network share: a look at a root
with a 3 s timeout, the indexer's walk with a round trip a folder, and
the archive copies of §197, all latency-bound with many small waits.
That is async's home ground for sockets. It is not for files: on
Linux, tokio's file IO is a thread pool underneath (`spawn_blocking`),
so an async rewrite would keep the same threads and the same round
trips under new names. io_uring would change that, and is not worth a
dependency and a platform split for a raw editor. The roadmap's own
line for it is the right shape in synchronous terms: several reads in
flight at once on a network root, a timeout on each, a bounded pool of
std threads, cancellable by generation as the Looks already are.

**What adopting it would cost.** A runtime as a dependency; a second
scheduling model beside the event loop and the thread-and-channel
one, so every reader of the code has to know which they are in;
`Send` and `'static` bounds spreading into types that today are
`Rc<RefCell<State>>` on the window's thread; and a harder headless
test story, since the tests drive the window synchronously and would
need a runtime in the loop.

**When to change the answer.** If greycard ever holds many network
connections at once (parallel HTTP with progress, a sync protocol, a
tethering stream beside a download), async earns its place and Slint
has `spawn_local` to meet it. Nothing on the roadmap does that: the
model fetches are one file at a time, the cloud rule is a mount or a
configured command and never a provider's API (§195, §197), and the
archive copies are one stream at a time on purpose, latency being the
limit on a share.
