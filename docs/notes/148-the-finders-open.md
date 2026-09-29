# 148. The Finder's open (2026-09-22)

§140 left the Finder launching greycard with no file. The Finder
sends the file as an Apple Event, and AppKit turns the event into
`application:openURLs:` on the application's delegate. That delegate
is winit's, and winit 0.30 does not implement the method, so AppKit
drops the files. Slint gives no way to supply a delegate, and
replacing winit's would lose its launch and termination handling.
So `finder::install` adds the one method to winit's
`WinitApplicationDelegate` class at run time, with `class_addMethod`,
after Slint has built its event loop and before the loop runs. It
then sets the same delegate again, because AppKit may note which
methods a delegate implements when it is set. If a later winit
implements the method itself, `install` sees that, leaves winit's
method in place and logs a warning, so an upgrade cannot silently
replace one handler with the other.

The paths wait in a queue on the main thread. The launch's event
arrives before `applicationDidFinishLaunching:`, which is before the
window has a device to develop on. The rendering setup takes the
queue and, when it holds anything, opens those paths instead of the
folder the settings remembered. After that, each arrival is opened
from a zero-length timer, so the open never runs inside a borrow
that was held when the event came. The folder chooser at startup now
waits for the same kind of timer and does not appear when a file has
already arrived. On the other platforms that timer changes nothing
but the order.

An open goes through `open_paths`, which lists each path as the
command line's path is listed: a raw on its own, a sidecar as its
raw, and a folder as its pictures. A selection of several files
becomes one list in the Finder's order, with each file kept once;
`open_folder` and `open_paths` then share `open_files`. The objc
part type-checks and passes clippy against the real objc2 crates
for `aarch64-apple-darwin`, but nothing here has run on a Mac: the
first Mac build has to confirm a double-click, Open With and a drop
on the Dock icon, both at launch and while greycard is running.
