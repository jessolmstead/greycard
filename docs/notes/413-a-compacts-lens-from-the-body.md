# §413. A compact's lens from the body

*2026-10-23*

A tester's CHDK DNG from a PowerShot SX710 HS got no lens
correction with lensfun installed. The file names the body and no
lens, and the lookup took a lens only by the name in the file, so
the entry lensfun keeps for that body's own lens was never tried.
Every fixed-lens compact, phone, and action camera was in the same
place.

## Which mounts are a body's own

lensfun gives a compact a mount of its own, `canonSX710HS`, and
lists its lens on that mount. The database tells an interchangeable
mount by a `<mount>` element with its `<compat>` list; a compact's
mount has none. A mount is fixed-lens when it has no element of its
own, no element names it as compatible, and none of its lenses also
sits on a defined mount. Checked against the whole database: the 52
defined mounts are all interchangeable systems, and every undefined
mount with a camera on it is a compact, a phone, an action camera, a
drone, or the Olympus E-10 and E-20 bridge cameras. M39 is undefined
but compatible with a defined mount, which the second clause catches.

## Which lens, when there are several

With no lens named and the body known on a fixed mount, the
straight-line lenses on that mount that cover the focal length are
the candidates. One left is taken. Several left: a raw takes the
entries calibrated on CHDK's DNGs when any fit, since no such body
shoots raw without CHDK, and the plain entries are made from the
camera's own corrected JPEGs and are near null (an SX510's plain
profile has a k1 of -0.007 against the CHDK profile's a 0.094, b
-0.309, c 0.163). A picture that is not a raw never takes a CHDK
entry, since the camera corrected it already. Past that, the one
plain entry ("& compatibles" or "(Standard)") wins over a converter,
and two converters with no plain entry get none rather than a guess.
The reviewer found the CHDK half: the first version took the plain
entry on the four bodies that have both.

The `raw` flag runs from `Database::lookup` and `profile` (a raw
frame) and `lookup_shot` and `profile_for` (the callers say), through
the editor's decode worker, the reduced develop, and the CLI. A named
lens is matched as before and wins; a name the database does not
match, such as a focal range written as the lens, still gets nothing.

## Not done

- A lens name that is only a focal range ("4.5-135.0 mm", as older
  compact raws write) does not fall back to the body's lens.
- Thirty-one compact mounts have cameras in lensfun and no lens;
  those bodies still get none, which is right.
