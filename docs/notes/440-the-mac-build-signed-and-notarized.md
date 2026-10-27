# §440. The Mac build, signed and notarized

*2026-10-27*

Until now the bundle carried an ad-hoc signature, and a downloaded copy
was stopped by Gatekeeper: testers clicked through Privacy & Security,
or ran install.sh, which took the quarantine mark off. From 0.9.0 the
release signs it with a Developer ID and has Apple notarize it, so it
opens at the first double-click.

## Signing

`scripts/package.sh` signs with the identity `MACOS_SIGN_IDENTITY`
names, inside out: the two libraries and the command line, then the
bundle, which seals the editor (its main executable), Info.plist and
the resources. No `--deep`, which Apple advises against for signing
because it gives every nested piece the bundle's options. Every piece
has the hardened runtime and a secure timestamp; notarization refuses
anything without them. Unset, the script signs ad hoc as before, so a
local package needs nothing of Apple's.

No entitlements. ONNX Runtime and Dawn are loaded with dlopen from
inside the bundle, and signed by the same team, so library validation
passes them; neither JITs on the CPU, and Core ML and Metal need
nothing granted. A develop with the learned denoiser from the
notarized bundle loaded ONNX Runtime and ran the model on Core ML and
on WebGPU. The Mac build fetches no code at runtime (only models,
which are data), so nothing unsigned is ever loaded.

## Notarizing

With `MACOS_NOTARY_PROFILE` naming a notarytool keychain profile, the
script zips the signed bundle, submits it, waits, and staples the
ticket into the bundle before the tarball is rolled. Stapled, the
ticket survives the tarball and Gatekeeper passes the app offline.
A rejection prints Apple's log and fails the package.

The submit and the wait are separate calls. The first submission from
an account took over an hour, and a dropped connection ended the wait
while Apple went on with the submission; so a failed wait is retried
against the same submission id rather than uploading again.

## A disk image beside the tarball

Notarization does not need one: the ticket is inside the app, so the
tarball opens as cleanly. The image is for the install a Mac user
expects, a window with the app and Applications to drag it onto,
where a .tar.gz downloaded by anything but Safari is left as a file
they may not know to double-click. It holds only those two; the
tarball keeps install.sh for the `greycard` command, which would be
odd to run from a read-only volume, and stays the reproducible build,
since hdiutil puts a fresh UUID and the time in every image.

The image is HFS+, zlib at level 9, holding the app already stapled.
It is signed (no hardened runtime; that is for code) and notarized
itself, then stapled, so Gatekeeper checks it before it mounts. That
is a second submission a release, minutes once the account is known.
The release workflow mounts it, assesses the app inside, and runs the
command line from it before uploading.

## In the release workflow

The Mac leg builds a keychain of the run's own from two secrets, the
Developer ID as a .p12 and its password, and stores the notary's App
Store Connect API key (three more secrets) in it as the profile. An
API key rather than an Apple ID and app-specific password: it is not
the account's password, and it can be revoked alone. The partition
list lets codesign use the key without a dialog nobody is there to
click.

install.sh leaves the quarantine mark alone now; a notarized app
needs it gone no more than any other.
