# 8. Licensing

*Superseded 2026-09-05: everything in this repo is GPL-3.0-or-later, see
§13. `rawcolor` stays MIT OR Apache-2.0 in its own repo.*

- **Engine and every library crate: MIT OR Apache-2.0** (as `rawcolor`).
  This is what makes library-first work: RapidRAW, darktable-adjacent tools or
  a commercial product can depend on it without a legal conversation.
- **The app: GPL-3.0-or-later.** Stops proprietary skins, matches what
  darktable/RawTherapee users expect, compatible with everything above. Not
  AGPL: the network clause does nothing for a desktop editor and scares
  packagers.
- **rawler is LGPL-2.1** and Rust links statically. An MIT engine crate can
  depend on it, but any binary must let users relink against a modified rawler
  (ship object files or a dynamic library, or be GPL). The GPL app is fine; a
  permissive downstream consumer of the engine inherits the obligation. One
  reason a native decoder might eventually be worth writing.
- RapidRAW is AGPL-3.0 with one dominant copyright holder, so its code cannot
  come here and nothing here can go there.

---
