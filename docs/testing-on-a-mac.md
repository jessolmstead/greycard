# Testing greycard on a Mac

For anyone trying greycard on a Mac. Ten minutes the first time, two
after that. Nothing here needs a terminal unless you want one.

## What you need

- A Mac with an Apple chip: any MacBook Air or Pro, iMac, Mac mini
  or Mac Studio from 2020 on with M1, M2, M3, M4 or M5 in its name.
  Apple menu, About This Mac: the line that says **Chip**. If it
  says Intel, greycard has no build for it yet.
- macOS 13.4 (Ventura) or newer. Same window, the **macOS** line.
- A folder with some raw files in it. Canon, Fujifilm GFX, Sony,
  Panasonic and Nikon raws are all tested.

## Getting it

1. Open the link you were sent, or go to the releases page and find
   the newest release.
2. Under **Assets**, download the file whose name ends in
   `-aarch64-macos.dmg`. Ignore the two "Source code" links.

## Installing

1. Double-click the `.dmg` in Downloads. A window opens with
   greycard and a shortcut to Applications.
2. Drag greycard onto Applications.
3. Open greycard from Applications, Launchpad or Spotlight. macOS
   asks once whether to open an app downloaded from the internet;
   click **Open**. From now on it opens like any other app.
4. Eject the greycard disk in Finder's sidebar, and throw the `.dmg`
   away if you like.

**The `greycard` command.** For the command line in the terminal as
well, download the file ending in `-aarch64-macos.tar.gz` instead.
Safari unpacks it into a folder in Downloads; if you see the
`.tar.gz` file itself, double-click it. Then open Terminal (⌘ Space,
type `Terminal`, Return), type `cd ` with a space after it, drag the
folder onto the Terminal window, press Return, and type
`sh install.sh` and Return. That puts greycard in Applications and the
`greycard` command in `~/.local/bin`; `sh uninstall.sh` in the same
folder takes it all out again.

**Signed and notarized.** From 0.9.0 on, the app is signed with an
Apple Developer ID and checked by Apple before it is published, which
is why macOS opens it with no more than that one question. A release
before 0.9.0 is not, and comes as the `.tar.gz` only: macOS says
**Apple could not verify "greycard" is free of malware**, and it opens
only after **Open Anyway** under System Settings, Privacy & Security,
or after `sh install.sh`, which for those releases clears the mark
macOS puts on downloads.

## First run

greycard asks for a folder. Pick one with raw files in it. The first
picture opens, developed; the strip along the bottom is the rest of
the folder, and **G** lays the whole folder out as a contact sheet.

The panel on the right has the controls. **Export…** at its foot
writes a JPEG or TIFF, and asks where.

Some things fetch themselves on first use, after showing you their
license: the lens database (under a megabyte), and the learned
models for denoising and masking (5 to 200 MB each). They go under
your Library folder and are downloaded once.

## When something goes wrong

The most useful things you can send, in order:

1. **What you did and what you expected.** A sentence each.
2. **The log.** greycard keeps `greycard-ui.log` in
   `~/Library/Logs/greycard`: which version and which Mac, the
   graphics chip, each picture it opened and how long each step
   took, and whatever went wrong. To get there: in Finder, Go menu,
   Go to Folder…, paste `~/Library/Logs/greycard` and press Return.
   `greycard-ui.log` is the last run, `greycard-ui.log.1` the one
   before, in case you opened it again after the trouble. Attach it.
3. **The raw file**, if a picture renders wrong, along with the
   camera it came from. A picture that looks wrong is the single
   most valuable report there is.
4. **If it vanished** with no message, macOS wrote a crash report:
   the same Go to Folder with `~/Library/Logs/DiagnosticReports`,
   and the newest file starting with `greycard-ui`. Attach that too.
   The Console app (in Applications, Utilities) keeps the last words
   of anything that quit, too: type `greycard-ui` in its search box.
5. Which version: it is in the name of the folder you downloaded,
   or `greycard-ui --version` in a terminal.

The quickest way to send it: **Report a problem…** at the bottom of
greycard's left panel opens the bug report form in your browser with
the version, the macOS version and the graphics chip already filled
in, and shows the log in Finder beside it; drag it into the form.
Or open an issue at
<https://github.com/jessolmstead/greycard/issues/new/choose> and
pick the bug report form. If GitHub is one thing too many, email or
message whoever sent you the link with the same things attached.

## Updating

Download the new release and do the install again; when Finder asks
whether to replace the old greycard, say yes. Replacing it is fine:
your edits live beside your raw files, and your settings and the
models it fetched stay where they are.

## Removing it

Drag greycard out of Applications to the Trash, or, if you used
`install.sh`, run `sh uninstall.sh` from the downloaded folder.
Settings are in `~/Library/Application Support/greycard`, the
fetched models and lens database in `~/Library/Caches/greycard`, and
the logs in `~/Library/Logs/greycard`; none of those is removed for
you. Your edits are `.gcd` files beside
your raws and are yours.
