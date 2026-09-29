# 111. A log for testers who never open a terminal (2026-09-20)

The README's bug section said "run it from a terminal and paste what
it prints; there is no log file." The first Mac tester is not going
to run it from a terminal, and a report from them would have been
"it didn't work." Two things here for that: a log the editor keeps
on its own, and a guide that walks a Mac user through the download,
the security dialog and what to send.

**The log is stderr, not a second channel.** Seventeen thousand
lines of `eprintln!` were not going to be rewritten to a logging
macro, and a macro would have missed what wgpu and ONNX Runtime print
from C, and the panic hook. So `log::start` makes fd 2 itself a pipe:
`dup` keeps the terminal, `dup2` puts the pipe's write end in fd 2,
and one thread copies whatever arrives to both the terminal and the
file, flushing the file on every read so a crash a moment later
loses nothing already written. Everything that was ever printed lands
in the file unchanged, the command line unchanged too. The `Log`
handle lives in `main`'s frame; dropping it puts the terminal back as
fd 2, which closes the pipe's last writer, and joins the copier, so
the last line before an exit reaches the file — a panic's message
included, since a panic in the main thread unwinds through `main` and
drops it on the way. The editor has no `process::exit` anywhere; it
quits through the event loop, so `main` always returns.

**Where, and how many.** `~/Library/Logs/greycard` on macOS, which is
where Console.app and every Mac user's "Go to Folder" habit look;
`~/.local/state/greycard` on Linux, the XDG state directory, which is
what that directory is for; `$XDG_STATE_HOME` wins on both. One file
per run, the previous run kept as `.1`: a tester whose editor
vanished opens it again before thinking of a log, and the `.1` is the
run that matters. The first line says the version, OS and
architecture, so a report that is only the file still says what was
run. Windows has no `dup2` and gets no log yet rather than a file
with only a header in it. `libc` is a `cfg(unix)` dependency for the
two calls; `std::io::pipe` is the standard library's since 1.87.

**The guide.** `docs/testing-on-a-mac.md` is for the person who has
not done this before: which Macs, the two ways to install with the
security dialog's exact words and buttons, what fetches itself, and
what to send when something goes wrong, the log and the crash report
and the raw file, in order of usefulness. It is written to be sent
with the download link and is linked from the README's Mac paragraph.

**Left for the roadmap.** A *Report a problem…* item in the editor
that opens the issue form with the version, the OS version and the
GPU filled in, and says where the log is. That is the piece that
turns a nontechnical tester's shrug into a report; the log is what
makes the report worth reading.

**What the log does not have yet.** Our own lines are not the
interesting ones. rawler, naga, wgpu, ureq and rfd log through the
`log` facade and ort through `tracing`, and nothing in the tree
installs a subscriber, so a wgpu validation failure, a rawler decode
warning or ort refusing a provider and falling back to the CPU is
silent today. That is the diagnostic a "it's slow" or "it's black"
report needs, and it already exists. A subscriber is twenty lines
and no migration; it is on the roadmap for 0.1.0 with the move of
the 82 `eprintln!` sites to the facade, so our lines get levels and
a `--verbose` too. The tee stays underneath either way: it is the
only thing that catches a panic's message and what C prints.
