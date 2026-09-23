# Testing greycard on a Mac

For anyone trying greycard on a Mac before it is in the App Store or
signed by Apple, which it is not yet. Ten minutes the first time, two
after that. Nothing here needs a terminal unless you want one.

## What you need

- A Mac with an Apple chip: any MacBook Air or Pro, iMac, Mac mini
  or Mac Studio from 2020 on with M1, M2, M3, M4 or M5 in its name.
  Apple menu, About This Mac: the line that says **Chip**. If it
  says Intel, greycard has no build for it yet.
- macOS 13.4 (Ventura) or newer. Same window, the **macOS** line.
- A folder with some raw files in it. Canon `.CR3` and Fujifilm
  `.RAF` are what it is developed against; most others open too.

## Getting it

1. Open the link you were sent, or go to the releases page and find
   the newest release.
2. Under **Assets**, download the file whose name ends in
   `-aarch64-macos.tar.gz`. Ignore the two "Source code" links.
3. Safari unpacks it on its own into a folder in Downloads named
   like `greycard-0.1.0-aarch64-macos`. If you see a `.tar.gz` file
   there instead, double-click it and the folder appears.

Inside the folder: `greycard.app`, two small scripts, the license
and a README.

## Installing

Two ways. The first needs no terminal; the second skips the security
steps.

### Without a terminal

1. Open a second Finder window at Applications (Go menu,
   Applications) and drag `greycard.app` into it.
2. Double-click greycard. A dialog says **Apple could not verify
   "greycard" is free of malware**. Click **Done**, not Move to
   Trash. This is macOS saying the app is not signed by Apple, which
   is true and is explained below.
3. Open **System Settings**, then **Privacy & Security**, and scroll
   down to the **Security** heading. A line says *"greycard" was
   blocked to protect your Mac*, with an **Open Anyway** button.
   Click it, enter your password or use Touch ID, and click **Open
   Anyway** again in the dialog that follows.
4. greycard opens. From now on it opens like any other app, until
   you install a newer version, when steps 2 and 3 happen once more.

### With a terminal

1. Open Terminal: press ⌘ Space, type `Terminal`, press Return.
2. Type `cd ` with a space after it, drag the unpacked folder from
   Finder onto the Terminal window, and press Return.
3. Type `sh install.sh` and press Return.

That puts greycard in your Applications folder, clears the security
mark so there is no dialog, and adds a `greycard` command for the
terminal. `sh uninstall.sh` in the same folder takes it all out
again.

**Why the security dialog.** Apps that skip it are signed with an
Apple developer certificate and checked by Apple. greycard is open
source under the GPL and is not, yet. The dialog means "Apple has
not looked at this", not "this is harmful"; you can read every line
of it on GitHub. `install.sh` removes the mark macOS puts on
downloads, which is what Apple's own check would have done.

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

Download the new release and do the install again. Installing over
the old one is fine; your edits live beside your raw files, and your
settings and the models it fetched stay where they are.

## Removing it

`sh uninstall.sh` from the downloaded folder, or drag greycard out
of Applications. Settings are in `~/Library/Application
Support/greycard`, the fetched models and lens database in
`~/Library/Caches/greycard`, and the logs in `~/Library/Logs/greycard`;
none of those is removed for you. Your edits are `.gcd` files beside
your raws and are yours.
