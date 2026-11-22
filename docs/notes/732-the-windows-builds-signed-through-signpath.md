# §732. The Windows builds signed through SignPath (2026-11-22)

The Windows executables and the installer are signed with SignPath
Foundation's certificate for open source. Nothing is signed on the
runner and no key is ours to hold: each signing request is an artifact
of the workflow's run, which SignPath fetches from GitHub, checks came
from this repository's workflow on a GitHub-hosted runner, signs, and
hands back (`.github/actions/signpath`).

## Two requests a leg

SignPath cannot open an Inno Setup installer to sign what is inside
it. So each Windows leg sends two requests, one after the other:

1. `windows-binaries`: `greycard-ui.exe`, `greycard.exe`, and Inno
   Setup's uninstaller. Inno writes the uninstaller apart in its signed
   uninstaller mode: the first compile stops once it has written it,
   and the second copies its signature in (`scripts/signpath.sh`).
2. `windows-setup`: the setup that second compile makes.

The signed executables go back into the zip, and a check holds that the
zip differs from the unsigned one in those two files alone. A release
has two Windows legs, so four requests.

The artifact configurations are kept with the code
(`packaging/signpath`) and pasted into SignPath; the two are changed
together. Each checks the product name and version of every file, as
the Foundation's terms ask.

## What is not signed

The DLLs beside the executables: Microsoft's, ONNX Runtime's, and
Dawn's. The Foundation signs a project's own builds alone, and its
terms allow others' binaries to ship unsigned inside a signed package.

## Two policies

- `release-signing`: a tag's run. A person approves each request on
  SignPath, as the terms require of every release.
- `test-signing`: the package workflows run by hand, from any branch,
  with a test certificate no Windows trusts. It tries the whole chain.

Without the `SIGNPATH_API_TOKEN` secret the signing step says so and
the builds go out unsigned, so a fork's workflows still run. With it,
every request has to come back signed and checked or the leg fails: a
release never ships unsigned in a signed one's place. On x64 that fails
the release; on ARM, which may fail, it leaves that leg's packages out.

## What a tester sees

The publisher Windows shows is SignPath Foundation. SmartScreen may
still warn on a first download until the file gains reputation;
signing every release with one certificate lets that carry across
versions.
