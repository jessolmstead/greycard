# 112. The log done the right way (2026-09-20)

§111 got a log to the first Mac tester in an afternoon by making fd 2
a pipe and copying it. Asked what the right way was, the answer was
that the tee is a hack in exactly one sense: it captures output by
hijacking the process's descriptor instead of asking each source for
it. There are four sources and each has a proper destination, and
none of them needs to know where fd 2 points.

**What the tee cost.** A pipe holds 64 KiB; with the copier starved
or dead, every write to stderr in the process blocks, and a panic in
that state deadlocks instead of reporting. It is process-global state
that the test harness, a debugger and a second instance all disagree
with; the test-only special case in the same file was the tell. It
forks the design by platform, since Windows has no `dup2`, so the
Windows log stayed open no matter what else landed. And it makes the
file a transcript of the terminal, which ties the file's verbosity to
the terminal's forever: a file at info and a terminal at warn is not
expressible.

**The four sources.** Our own lines go through a facade, `log` in
the library crates (the community norm, no opinion in core) and
`tracing` in the binaries, with `tracing-log` carrying the `log` side
over losslessly (the other direction drops spans). The libraries,
wgpu, naga, rawler, ureq, rfd, zbus, already speak `log`. ONNX
Runtime's C logger is forwarded into `tracing` by the ort crate,
with a span per session and the C message as the event, so a
provider refusing to load and Dawn's device errors that the WebGPU
provider catches arrive as events with a target. Panics go through a
hook that formats the message, the location, the thread and a
`Backtrace::force_capture()` into one error event; that is the
explicit form of what the tee got by accident from the default hook,
and it costs nothing until a panic. What is left is a library calling
`fprintf(stderr)` directly, and the operating system already keeps
that: a Mac app launched from Finder has its stderr in the unified
log, which Console.app reads, and a Linux desktop launch lands in the
user journal. The tester guide names Console.app for the rare case.
If a library turns out to spam raw stderr with something needed, the
fix is that library's callback, not a global redirect.

**The subscriber.** `tracing-subscriber` with two `fmt` layers on one
registry, each with its own `EnvFilter`: the file at info, the
terminal at warn, both with the libraries at warn and our five crates
raised. `-v` brings the terminal to info, `-vv` both to debug;
`RUST_LOG`, when set, is taken for both sinks as it is. The file
sink is a `Mutex<File>` behind a `MakeWriter` of our own, because
tracing-subscriber's stock one panics on a poisoned lock and the
panic hook writes through it: a panic while writing must not become
a second one. The file gets seconds since start to the millisecond,
the thread name and the target; the terminal gets the level and the
message, which is what the eprintln lines looked like. `LogTracer`
is installed with a max level of info unless `RUST_LOG` or `-vv`
asks for more, so wgpu's trace lines cost a level check and nothing
else. The header line is written straight to the file before the
subscriber exists, so a report that is only the file still says what
was run. The CLI installs the same thing to the terminal alone: a
tool's stderr is its log. A wrinkle the CLI sweep found: the tool's
binary is named `greycard`, so its own events carry that target and
not the package's `greycard_cli`; a directive matches a target only
up to a `::`, so `greycard=info` does not take `greycard_core` with
it. arboard warns twice at every start that the compositor has no
data-control protocol, on every GNOME desktop, so that target is
held to error by default.

**Worker panics.** Each job runs under `catch_unwind`. A panic drops
the base, the learned result and the last develop, then delivers the
failed outcome that job was waited on for, `Failed`, `ExportFailed`
or `MaskFailed` by blame, with the message prefixed "the worker
panicked"; thumbnails and fetches blame nobody. The hook has written
the backtrace already. The editor's status line says why and the
busy state clears, where before it said "developing..." until closed.

**What the log says now.** The rule for levels: error is the
operation the user asked for failing; warn is something degraded,
skipped or silently fallen back on; info is what a bug report needs,
once per event; debug is detail for us. Nothing that fires per tile,
per frame or per thumbnail is info. At start: version, OS and
architecture; the settings path; the GPU's name, backend, device type
and driver from `Device::adapter_info()` in Slint's rendering setup,
or an error if the API is not wgpu, which is the "black viewport"
report's line; the display profile and where it came from; the model
store; the lens database with its counts. Per picture: what rawler
found (make, model, size, mosaic, black and white levels; the
as-shot coefficients and illuminants at debug), "opened" with the
seconds, the lens matched or a warn that it is unknown. Per develop:
one line with base made or kept, the denoiser's provider and
seconds or why it did not run, the fills, local contrast, dehaze,
sharpen, and the total. Exports say the path and seconds at info
and warn when a file was renamed or written over or skipped. The
silent fallbacks that warn now: no white balance or color matrix in
the file, a provider that would not load (with the first line of
why), the denoise cache or settings not written, a sidecar that
would not parse, a dump under a `GREYCARD_UI_*` variable that failed
to save. The `GREYCARD_UI_TIMING` frame line is info, since the
variable is the gate. The CLI's `develop` used to print nine
diagnostic lines unasked; they are `-v` now, its stdout is
byte-identical, and the warns it gained (an existing file renamed or
written over, `--ai-denoise` ignored on a rendered picture, CA left
uncorrected, no lens profile) show by default.

**Reviewed.** Each branch had a fresh reviewer. The editor's review
caught the frame-timing line at debug, which reached neither sink
and killed the variable; the GPU block with no line for the case it
exists for; two failures (the learned denoiser, a fill) that reached
the status line only; a skipped export at info; two warns in the
preview white-balance path that fire per frame during a slider drag,
now debug since the decode warns once per file; and the failed
outcome's line inside the generation check. The CLI's review caught a
warn split from its subject, "body not in the lens database" with no
lens named, and a bare "kept in" left as a print. The worker's
review caught the one that mattered most: a panic inside the develop
that follows an open left the worker's input on the previous file
while the UI, which had its Opened already, was on the new one, so
the next slider move developed the old picture under the new name;
the file is the worker's before the develop now. Also the fetch
threads, which run outside the job loop and were not covered; the
develop line's total taken before the half conversion the outcome's
seconds include; a missing model warned on every develop when it is
a state the line names and the UI offers to fetch; the same fill
failure warned in two places; a library claiming error for what its
caller recovers from; "no white balance" at warn when the develop
fails outright and says so; and the stamp rewinding with the base
while the mask caches keyed by it survived.

**Windows.** The same code: the subscriber opens
`%LOCALAPPDATA%\greycard\logs\greycard-ui.log` and writes it, and
nothing in the path is platform-specific, which is why the tee had
to go before Windows could have a log at all. The gap is raw C
stderr: once the editor is a GUI-subsystem program there is no
stderr, and Windows has no journal or Console.app to keep what a
library prints past the facade. ONNX Runtime's output goes through
its logger and wgpu's through `log`, so what is lost is whatever
Dawn prints on its own; Dawn has a log callback, and wiring it to
the facade is the fix on all three platforms if it is ever needed.

**Left.** *Report a problem…* in the editor is on the roadmap still;
the log is what makes it worth building. The tee is deleted, and with
it libc.
