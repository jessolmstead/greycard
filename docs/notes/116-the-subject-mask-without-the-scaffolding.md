# 116. The Subject mask without the scaffolding (2026-09-20)

§97's first two fixes, done: the last provider in the list skips its
warm-up when it is CPU, and a WebGPU (or CUDA) *failure* is remembered
on disk so it is paid once, not once a launch. The graph rewrite (its
third fix) stays on the roadmap. Landed in two passes on the same
branch — the second a review's fixes, kept together here rather than
as a separate entry.

**The last provider.** `runtime::try_open` took a `skip_warm` flag;
`runtime::open` sets it only when a provider is both last in the slice
and CPU: `skip_warm(last, provider) = last && provider == Provider::Cpu`.
CPU cannot accept a graph and then fail on the first real run the way
WebGPU can, so once its session builds a warm-up run proves nothing
that build did not already prove — but the check is on CPU itself, not
on position alone, since the load functions are `pub` and take an
arbitrary slice: `&[Provider::WebGpu]` alone is still warmed up, being
last but not CPU. The decision is a pure function, tested without a
model.

**The remembered failure.** A new file beside `runtime.rs`,
`runtime/record.rs`, keeps one JSON file beside the model cache:
`<store root>/providers.json`, e.g.
`~/.cache/greycard/models/providers.json` on this machine, a sibling
of the per-model directories `store.rs` already makes. Deleting it
clears every remembered answer; nothing else reads or writes it, and a
launch with the file missing, unreadable or holding nonsense just
probes fresh, the same as before this landed. A remembered entry only
ever turns into *skipping a provider entirely*, never into skipping
its warm-up: a remembered success still gets a fresh warm-up run, on
the reasoning a review of the first pass raised — skipping it would
hand back a session that has not actually run the graph this launch,
and if it then failed at real use (a silent WebGPU allocation
failure, which `denoise.rs`'s `Implausible` already exists to catch
once; a driver update that still reports the same adapter string;
another process holding the GPU) there is no provider left to fall
through to and nothing writes the failure back, so the bad session
sits cached in `ai.rs` for the rest of the editor's run. Recording a
success still costs nothing, so it is kept — a possible later win, if
a caller someday has a way to invalidate a remembered success on a
run-time failure rather than only on a load-time one. Until then the
whole gain here is in not re-discovering a *failure*, which is where
§97 put it.

What an entry is keyed on: the specific *file's* published sha256
(SAM's encoder and decoder are two files under one registry id, so
each gets its own slot — an earlier version keyed on the model id
alone and let the two stomp each other, caught by a test before it
shipped) and the provider. What makes a found entry still trusted,
checked separately (`record::valid`) rather than folded into the key,
so a driver update replaces the one entry rather than leaving an
orphan beside it: the adapter's identity and the build's own
fingerprint. Neither `ort` nor ONNX Runtime's execution providers say
which adapter they ran on, so this asks a second, short-lived
`wgpu::Instance` of its own, probed with `Backends::PRIMARY` (Vulkan,
Metal, DX12 — closer to what Dawn itself offers than every backend
wgpu knows, and no GL/EGL context opened in a process that may already
hold Slint's own wgpu device), for WebGPU's `AdapterInfo` (name,
backend, driver, driver info — the same struct `greycard-ui`'s
viewport already logs off its Slint device); for CUDA, shells to
`nvidia-smi --query-gpu=name,driver_version`, since neither `ort` nor
CUDA itself exposes it either — untested here, this build carries no
`cuda` feature and the machine has no CUDA 13. `None` on anything that
does not look like an answer, same as WebGPU with no adapter: the
provider is simply not remembered that launch. Each probe is asked
once a process and kept in a `OnceLock`, not once a model file: five
files a session (Subject, SAM's two, Fill, the denoiser) used to mean
five fresh `wgpu::Instance`s before this; a review caught it, since
building one is real work — enumerating backends, picking an adapter —
that the record was supposed to be saving, not spending again. The
`Instant` that times a provider now starts before that probe rather
than after, so the logged duration is honest about the full cost of
settling on a provider, not just the session build that follows it.

The build's fingerprint is the provider names in order, the crate's
own `CARGO_PKG_VERSION`, and now also `ort::info()`'s build string
(git branch, commit, build type — a cheap read of something ONNX
Runtime already has, no session needed), so a `cuda` feature added, a
different provider set available this run, or a greycard build
carrying a fixed or regressed ONNX Runtime at the same crate version,
each drop the remembered answer rather than trust a stale one.
Reading and writing is read-modify-write with no lock, matching the
denoiser cache's own "never worth an error" stance in `cache.rs`, and
a write is skipped outright when the entry about to be upserted is
already the one on disk — nothing changed, nothing to race another
process over. The temp file a write renames from now carries its own
process id and a counter, not one fixed name, so the editor and a CLI
run (or two editor windows) writing at once no longer tear each
other's temp file, and it is removed on any failure along the way
rather than left behind. Running the six ignored model tests together
(they load different models in parallel threads) still lost one
model's update to another's before this landed — a genuine race in
the read-modify-write, not the temp-file tear the fix above closes —
confirmed harmless by rerunning that test alone: a lost update only
costs a slower next launch, never a wrong answer. The "remembered
failing" log line now names the record's path, so a tester who sees it
knows what file to delete.

**Measured**, this machine, an RTX 5070 Ti, `GREYCARD_MODELS` pointed
at the real store (`~/.cache/greycard/models`) so the ignored test
actually loads BiRefNet, release build, the disc fixture from
`tests/models.rs`. The second pass's numbers were taken with the
machine considerably busier (other work on the same box; load average
around 30 on 32 cores, against a quiet machine for the first pass), so
the mask-run column is noisier than the load column and not really
comparable between passes — the load column, all CPU-bound session
work with no GPU contention, held steady across both:

| | load | mask | total |
|---|---|---|---|
| before (git master) | 5.17 s (WebGPU 1.27 s fail + CPU build 1.1 s + warm-up 3.2 s) | 2.81 s | 7.98 s |
| after, record cleared (fix 1 only pays) | 2.4–2.9 s (WebGPU ~1.3 s fail, no CPU warm-up) | 3.0–3.7 s | 5.4–6.6 s |
| after, second launch (both fixes pay) | 1.14–1.31 s (CPU build alone, WebGPU skipped) | 2.9–5.1 s | 4.2–6.3 s |

The load column is the number this pair of fixes actually owns, and it
repeats: 1.14, 1.17, 1.21, 1.26, 1.31 s across five separate runs on
two different days. Ten seconds to about four to five, as §97
predicted, with the model itself unchanged; the mask-run spread on a
busy machine is scheduling noise the fixes have no say over. SAM's two
files still run on WebGPU without the Split bug at all; unlike the
first pass, its second load no longer drops to near-zero, since a
remembered success no longer skips the warm-up — it repeats at 1.16 s
to 1.42 s, both files warmed, both launches, which is the cost of the
review's fix and the number worth remembering if that skip is ever
brought back under a run-time invalidation.

**Tests**, none needing a model: in `runtime/record.rs`, a round trip,
a missing-or-broken file reading as empty, `find` matching a file's
hash and provider (not the model id alone — the SAM collision above),
`upsert` replacing a slot in place and adding a new one for a
different file, a changed adapter or build invalidating an entry, two
files under one model not sharing a slot, and the build fingerprint
naming the providers in order. In `runtime.rs`: `skip_warm`'s cases —
CPU last skips, WebGPU or CUDA last does not, CPU not last does not —
a load with nowhere to remember to (`remember: None`) touching no such
file, and `plan` (the lookup `open` hands its decision to: given the
entries already read, a file's hash, a provider, and this launch's
adapter and build, what outcome to trust, if any) trusting a matching
entry and ignoring one whose adapter, build, provider or file
differs. One more test drives `plan` against a real file written to a
fake store root (a temp directory, not `~/.cache`) with a hand-seeded
failure, and checks the file's bytes are unchanged after — the
"remembered failure is skipped and nothing is rewritten" case, as
close to `open`'s own wiring as could be reached without asking real
hardware for an adapter. Driving `open` itself through that path was
not attempted: `Provider::adapter_identity` has no seam to hand it a
fake answer without either a test-only override baked into the enum's
real, hardware-backed probes, or an adapter override threaded through
`open`'s public signature — both change an API only this one test
would use. `plan` is exactly the decision `open` hands the entries and
the adapter to, so the test reaches the same logic over the same data;
the full path (including that `Provider::adapter_identity` really did
find the right cached string) is what the ignored model tests above
checked by hand, twice, seeing the record read and the correct
provider chosen.

**Left out.** The graph rewrite itself (§97's third fix, a separate
roadmap line) — BiRefNet on WebGPU is still `Failed` on this machine,
now remembered rather than re-discovered. CUDA is wired the same way
mechanically but not exercised: no `cuda` feature build and no CUDA
toolkit here to test the `nvidia-smi` probe against a real failure or
success. Skipping the warm-up on a remembered success (SAM's WebGPU
load fell to 0.50 s under that rule in the first pass) is left out per
the review above, kept only as a note here in case a later invalidate-
on-run-time-failure path makes it safe to bring back.
`crates/greycard-cli`'s learned-denoise path now goes through
`Denoiser::from_store` when it has a registered tier in hand (it
always did have the `Model`, just was not using it, so the CLI never
read or wrote the record before this), matching what `greycard-ui`'s
`ai.rs` already does. `greycard-ai` gained three dependencies for
this: `serde` and `serde_json` (already in the workspace, for
`providers.json`), and, behind the `webgpu` feature only, `wgpu`
(already resolved at the version `greycard-ui` gets through Slint, so
nothing new to fetch) and `pollster` to block on its one
`request_adapter` call, now asked once a process rather than once a
model file.
