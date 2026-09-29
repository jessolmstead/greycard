# 136. Its own name and domain (2026-09-21)

greycard has a domain, greycard.org, and two identifiers were rebound
onto it while they are still free to rebind.

The bundle identifier was shaped `io.github.<account>.greycard`,
which ties the application to wherever its source happens to be
hosted; it is now `org.greycard.greycard`. This is the last cheap
moment for that change. Apple's App ID and every notarization record
attach to the identifier, and once a signed release is out macOS
treats a changed one as a different application altogether —
different preferences, a different container, and an upgrader who
ends up with two copies. Nothing is signed yet, so it costs nothing
today and would cost users something later.

The XMP namespace moved the same way and for the same reason, from a
hosting URL to `https://greycard.org/ns/1.0/`. It goes into the
packet of every file greycard exports, so it belongs to the project
rather than to an account. The change is cheaper than it looks:
of the properties the sidecar code owns, only the pick flag lives in
greycard's namespace — rating, label, keywords, title, description
and orientation are all in Adobe's — so a sidecar written before
this loses a pick flag and nothing else, and reading both URIs would
recover even that.
