# §449. The panel's choices written as they change

*2026-10-29.* On a Mac the Canvas, Monitor, and Soft proof choices
came back to their defaults on relaunch (issue 55). They, and every
other choice read from the panel rather than written as it is made,
were gathered and saved after the window's event loop returned. On a
Mac, Quit from the menu or Cmd+Q is AppKit's terminate: winit's
will-terminate handler sends the loop its exiting event and hands
back to AppKit, which ends the process, and `run()` never returns.
Nothing after it runs. The reporter's log ends mid-run with no close
line, which is what that looks like. Closing the window with its
button went through the loop's end and kept everything.

## The write

The gather, the merge with the fields the file already holds (the
ones written as they change, such as the sidecar switches and the
last file), and the save are one function the close path and a timer
share. The timer runs once a second, reads the panel, and writes only
when the gathered choices differ from the last write, one debug line
a write; a file that will not parse is left alone with one warning
rather than replaced with defaults mid-run. The loop's exiting event,
which winit sends on every platform and the Slint fork passes to the
application handler we already install, saves once more, and flushes
the open frame's pending edit, whose own save timer is 800 ms and
would have been lost the same way. The settings file is now written
by a temporary file beside it, named with the process id, carrying
the old file's permissions, renamed over the resolved target so a
symlinked file stays a link.

## What is still skipped on a Mac's Quit

Everything else after the loop: the typed title's send, an import's
stop after its file, the worker's stop, the index connections' close
and the write-ahead log's fold. The worker may be in a job when the
process ends. Moving the rest of the close into the exiting hook is
a follow-up on the roadmap.
