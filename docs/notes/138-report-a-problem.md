# 138. Report a problem… (2026-09-22)

§111 left one piece for the roadmap: an item in the editor that opens
the issue form with what a tester cannot be expected to know already
filled in, and that says where the log is. It is a button at the foot
of the left panel, "Report a problem...", where it is looked for and
away from the work. It opens the bug form in the browser, shows the
log selected in the file manager beside it, and the status line says
to drag `greycard-ui.log` into the form.

**The URL is the whole mechanism.** GitHub fills an issue form's
fields from query parameters named by the fields' ids, so
`?template=bug.yml&version=…&os=…&gpu=…&log=…` is all there is to it.
Nothing is sent from the editor: the tester reads the form, adds to
it, and submits it or does not. That makes the ids in `bug.yml` a
contract with `report.rs`, and the form says so in a comment at its
top. The form needed two changes anyway. "Distro and desktop" assumed
Linux, so it is `os` now. "Terminal output … there is no log file" had
been wrong since the log landed, so it is `log` now, with no `render:`
key, because a rendered field refuses attachments. Each value is
percent-encoded as UTF-8 and held to a kilobyte once encoded, cut at a
whole character, so a driver string can say what it likes and the URL
stays far inside what browsers and GitHub take.

**What is filled in, and from where.** The version is the crate's. The
GPU is the line the log's start already wrote, from the adapter Slint's
rendering setup found, kept on the state; when the API is not wgpu it
says so, which is the black-viewport report's answer. The OS is each
platform's own account of itself. On Windows that is `RtlGetVersion`,
because `GetVersionEx` answers 6.2 to a program without a compatibility
manifest, and Windows 11 still calls itself 10.0 and is told apart by
build 22000 onwards. On macOS it is `sw_vers`. On Linux it is
`PRETTY_NAME` from os-release, the kernel, and the desktop and session,
the last being what the old form asked a tester to know. The
architecture goes on the end of all three, which on a Mac is the Apple
silicon question. No new crate: the Windows call is a `raw-dylib`
extern of a dozen lines, and `webbrowser` was already in the tree
under Slint's winit backend.

**The log's path, without the name in it.** The form is public and the
path carries the account name, so it is written as
`%LOCALAPPDATA%\greycard\logs\greycard-ui.log` or
`~/Library/Logs/greycard/greycard-ui.log`. The field also names the
`.1`: a tester whose editor vanished opens it again to press this
button, and that start moved the run that went wrong to
`greycard-ui.log.1`. The file that matters most is the one they would
not think to send.

**Showing the file, not only the folder.** Explorer's `/select`,
Finder's `open -R`, and on Linux the file manager's `ShowItems` over
D-Bus, which Nautilus, Dolphin, Nemo and Thunar answer, with `xdg-open`
on the folder when none does. Explorer parses its own command line and
wants the quote after the comma, so the argument goes in raw. The
browser and the file manager are asked off the event loop, since
`xdg-open` waits.

**Checked, and not.** Seven tests on the URL's shape and escaping, the
cut at a whole character, a 10,000-character field staying under
8 KiB, the path's abbreviation, and the three OS parsers on fixed
input; `os()` on the Windows machine returned "Windows 11 10.0.26200,
x86_64", which it is. The button is drawn and the GPU line unchanged
in a snapshot run. The button was not pressed, since that opens
github.com, and the macOS and Linux branches were not compiled here;
the first CI run on those runners is their check. And it works only
once the repository is public with issues on and a `bug` label, and
the new `bug.yml` is on the default branch: until then the button
opens a 404 for anyone but the owner.
