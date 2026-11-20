# §723. A lens match that knows its line (2026-11-20)

**Asked.** A full-frame Leica M11 with a Summilux-M 35 got the profile
of the Summilux-TL 35, an APS-C lens of another design: the database
has no M-mount Summilux 35, and the lookup's last rung, which ignores
the mount, took the nearest name. A wrong correction is worse than
none.

**Decided.** The ladder, in order (`greycard-lens`, `matching.rs`):

1. The body is found by make and model; it gives the mount, the
   mounts that mount takes, and the crop factor.
2. The lens name is matched among straight-line lenses on those
   mounts that reach the focal length, names compared as token sets,
   0.4 the floor.
3. A three-digit code with a leading zero (a Sigma year, a Tamron
   model) is never a focal length: a Mark II's "024" is not the 24 of
   a 24-70. When both names carry codes they must agree, which tells
   Tamron's A056 from its A065. When only one does, the code is left
   out and the rest must agree to 0.6: under that it is another design
   of the maker's (an EX DG HSM against a DG DN Art scores 0.43).
4. A name carrying one of a maker's line words (Leica M, TL, SL, R,
   S; Canon RF, EF, FD; Nikon Z, F; Sony FE, E, DT) is not matched to
   that maker's lens of another line.
5. A file that names a first-party line is not matched to another
   maker's lens unless that lens carries the line's word: without the
   right sibling, any 50 mm f/1.8 would otherwise do.
6. Of entries of one name, the calibration that covers the body wins
   (lens crop at most the body's by 1.05), then the nearest. Ties
   between different names keep the database's order.
7. A name the same outright beats one the same once a code is left
   out.
8. Only when nothing on the mount fits is the name tried across every
   mount, for third-party glass on an adapter.
9. A refused match leaves no profile and the switches stand down; the
   picker still offers every lens by hand, and a lens chosen by hand
   stays.

`MATCHING` is 2 and is part of the lens database's name in the
develop key, so pictures kept from edits with a lens correction are
remade once.

**What we tried and took back.** Refusing, off the mount, a lens
calibrated on a smaller sensor (lensfun's own rule) turned away 53,178
pairs that matched before, right ones among them: a full-frame lens
lensfun calibrated only at 1.6, on another maker's body through an
adapter. The line word catches the Leica case alone, so coverage is
only the preference of rule 6. Letting a well-named lens off the
mount beat one on it sent native lenses to other designs on a small
difference of spelling (a Micro Four Thirds 14-42 to the Four Thirds
one), so the mount comes first, always.

**Measured** against the matcher before, over the whole database: 1,052
cameras and 1,568 lenses, each lens tried on bodies of its own,
compatible, and 16 other mounts, 208,302 pairs. A lens finding its own
entry: 163,238 before, 167,069 now, none lost. Each lens taken out of
the database and looked up by its own name, 14,821 lookups: none now
goes to another maker's lens (32 did in the first version), none to
its maker's lens of another design. Each native lens's name with one
word left out, 42,108 lookups: none leaves its mount. Over our own
sample raws, 13 frames change: the M11's (no profile now) and a Sigma
24-70 DG DN that took the Mark II's profile and takes its own.

**Left.** A lens name with no line word ("Summilux 1:1.4/35") on an M
still takes the TL. Sigma's and Tamron's own line words (DC and DG,
DN and HSM, Di II and III) are not in the table. 36 database entries
are bare numbers under a maker's name and match any bare name a body
writes. A lens the database lacks can still take a same-maker sibling
of its line where the names agree to 0.6.
