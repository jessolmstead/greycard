# 195. Immich as a neighbor, not a backend (2026-09-27)

Immich is a self-hosted photo library with no raw developer, the same
gap digiKam has in `where-greycard-fits.md`, and its users are
the people that document is written for: folders they own, no catalog
held hostage. The two meet at the folder, so the integration is mostly
already there; what is worth building is small, and one thing is worth
refusing.

**The shared folder, no code.** Immich's external libraries index a
mounted folder in place, read-only, and read an `IMG.CR3.xmp` or
`IMG.xmp` beside each file. greycard's folders are the truth (§72), its
XMPs stay beside the raw when the `.gcd` sidecars move into `.greycard`
(§153), and with `xmp_sidecars` on it writes rating, label, keywords,
title and caption into them. Pointed at the same NAS root, Immich
should show greycard's meta with no work on either side. Should, not
does: which XMP fields Immich maps is unchecked (keywords in
particular, where `dc:subject`, `lr:hierarchicalSubject` and a
`TagsList` are all candidates), and so is whether Immich writes back
into a sidecar under an external library. If it does, greycard already
reads a sidecar another program wrote and settles the two by their
clocks, so a second writer is a case it was built for. The check is an
Immich in a container against a copied shoot with greycard-written
XMPs, an hour's work, and it decides the rest.

**Exports into Immich.** Immich's picture of a raw is the camera's
embedded JPEG or a plain LibRaw develop, so a greycard edit is
invisible there. Two ways to fix that, in order of cost:

- An export preset whose destination is a folder Immich watches. The
  export already carries EXIF and an XMP packet (§40), so the date,
  the position and the rating come along. Needs only the destination
  folder in an export preset, which the pool already has.
- A post-export command, `immich upload` or anything else, run with
  the exported paths. The better end state is the export stacked over
  its raw in Immich as the stack's cover, so the edit is what is
  browsed and shared; that is the command's business, not greycard's.

The roadmap's rule for the cloud is a mount or a configured command,
never a provider's API, and it holds here for the same reasons: no
client to keep in step with a server that versions quickly, no
credentials in greycard's settings, and the same hook serves every
other destination.

**Refused: Immich as the library's data.** Immich runs face
recognition and CLIP search, and reading them through its API would
fill the faces facet for free. It would also make a running server a
dependency of the library and put data in the index that a rebuild
from disk cannot recover, which is the one thing §72 rules out. Faces
stay a local model writing names into the meta section as keywords,
and those keywords reach Immich through the XMP like any other.
