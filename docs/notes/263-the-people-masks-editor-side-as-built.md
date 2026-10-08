# 263. The People mask's editor side, as built (2026-10-08)

§253's design built into the editor, on §256's model side. The shape
was designed and built as "Parts"; after trying it the user named it
**People**, which is what every string the user reads now says. The
code keeps its names (`Shape::Part`, `PartKind`, `PARTS`, the
sidecar's `part` kind, the cache keys), so nothing on disk changed.

### The menu and the flow

**People** on the Masks tab opens a menu: **Whole person** alone at the
top, then Face (Face, Facial skin, Eyebrows, Eyes, Iris, Lips, Teeth),
Body (Skin, Hair, Hands) and Clothing (Top, Bottoms, Shoes, All
clothing). Its icon is two people, so it reads apart from Subject's
one. Picking an entry starts a worker job that finds the people on
the picture (`Job::People`):

- No one: the shape is made at once and is everyone's (`person: None`).
- One person: made at once, and it is **that person's** (below).
- Several: each face is outlined and the status line asks "click the
  person, or All people". A click makes the shape theirs; **All
  people** makes it everyone's (`None`); Esc makes nothing.

A shape stores its phrase as text, its route, and the `Person` it is
of: the signature (§256's `colors-1`), where the face was (`at`, in
the picture's units as turned, and turning with the shape), and the
picture it was made on (`greycard_library::hash_file`: BLAKE3 of the
first 64 KB and the size). A route this build does not know
(`Route::Other`) loads and saves unchanged, as a shape this build
cannot draw.

**Signatures from a look-neutral picture.** A person's colors are
read from the picture developed with no edit applied (`Edit::default()`)
and turned back to the camera's orientation, so a look, an exposure or
a turn never moves them. White balance, the camera profile, the lens,
noise reduction and demosaic are in that picture, so a change to one
of them can move the colors (below).

### Finding the person again

`settle` decides, for a shape with a person, which face on this
picture is theirs:

- **On the picture it was made on** (the hash matches), the face at
  `at`, if it is the same render or within BOUND. If the face is still
  there but its colors have moved past BOUND, as after a strong change
  of white balance, it **asks with that face as the guess**: one click
  settles it. §256's own rule still wins where it decides alone.
- **Anywhere else**, §256's `choose`: decided alone on a one-face
  picture within BOUND; otherwise a guess (BOUND, everyone comparable,
  MARGIN) or an ask with none.
- **Not found at all**, with people on the picture: it asks as on a
  group, with no guess, and the status line says "this mask was made
  for someone else: click a person to use it for them, or All people".
  The user's call: a pasted mask finds the same person and never takes
  whoever stands there, unlike Lightroom, but a mask made for someone
  else should be one click from being theirs, not a dead end. It is
  never a dialog, so a paste or sync onto many pictures never blocks:
  each picture asks when it is opened, and the outlines come back when
  the user returns to Masks.
- **No one on the picture**: the mask stays empty and the status line
  says "no one is in" the picture.

**The lone person is remembered.** First built as `None`, a shape made
on a one-person picture then masked everyone when pasted onto a
picture of two. The user chose to remember them: such a shape stores
its person as a pick does, so pasted onto another frame of them it
finds them, and onto someone else's portrait it asks. Someone dressed
very like them, inside BOUND, can still be taken for them; the same
woman measured 4.80 and 7.44 apart across frames against a BOUND of
7.5, so some of their own frames will ask.

A click on an ask stores the face's signature, place and this
picture's hash in that picture's copy of the shape, an ordinary pick
from then on. Asks and "nobody" are never cached, only a settled
raster; the turn is in every cache key, memory and disk, and
`PART_PIPELINE` is `part-2`. The model is let go when no live People
shape remains; a set export borrows it (`lend_parts`) rather than
loading its own. An export with a shape that hasn't settled writes the
mask empty and says why in its result line and the log ("a People
mask needs a person picked", "a People mask's person isn't in this
picture"); it never waits on an ask.

### Whole person

Asked for after trying the build: Subject takes everything salient and
cannot pick one person out of a group. **Whole person** is the preset
phrase `person` (entry 14 of the shipped table, already used by the
person match, so no new upload) on the whole route, with its own path
in `sam3::whole` (`person_mask`):

- the picked person's own body, the `person` instance §256 paired with
  their face, when it holds no other face; so a duplicate body never
  leaks in;
- else the instance over the cut that holds their face and no other;
- else, when SAM merged them with someone into one body, that body
  split between the faces it holds, each cell to the nearest face
  measured in face widths across and quarter face heights down. That
  suits people side by side and misassigns overlapping poses, a child
  on a lap; none of the trial's groups merged, so it is tested on
  synthetic masks only;
- else, with only a head showing, SAM's face mask for that face, or
  its box.

All people is the union of every `person` instance, faces or not, so
someone whose face SAM missed is in it; but a person is picked by
their face, so someone with their back turned can't be picked alone.

**Made solid.** SAM 3's values inside a body run about 0.7 to 0.95, and
through the refine step that made the inside blotchy, with dark
glasses a hole. For Whole person the values ramp from 0 at 0.3 to 1 at
0.7, centered on the cut, and a hole enclosed by the person is filled,
with a two-cell rim, unless some cell in it reads 0.1 or less. On the
trial's thirteen frames, real gaps (an arm against the body) read 0.01
to 0.04 and stay open; glasses and shadows read 0.13 to 0.45 and are
filled. The thresholds come from those frames only. A small enclosed
gap, hands on hips or feet together on a small full-length figure, may
never fall to 0.1 at SAM's 288 cells and be filled; a gap open to the
background or the frame's edge never is. The other whole-route parts
(Top, Hair...) keep the old path and likely the same uneven inside.

**Against Subject.** On nine trial frames, All people against Subject:
IoU 0.95 to 0.99 on most, 0.76 on a portrait where Whole person
rightly leaves out lavender in front of her, 0.82 to 0.88 on three
more. The soft band at the edge is 1.3 to 1.6 times Subject's (12 to
27 pixels per edge pixel between 0.1 and 0.9 at 2048 wide, against 10
to 20): close on plain hair, but wisps against the sky that Subject
keeps are lost. Telling people apart is where it wins: a couple comes
out as two people, her arm across his chest hers. Not built: inside
the picked person's region grown a few cells, take Subject's matte as
the edge, for BiRefNet's hair at one more model run per picture
(about 0.2 s on WebGPU, its matte already kept) and its 114 MB
download; where two people touch, the edge between them stays the
person split.

### Tests

On the trial's frames on WebGPU (`parts_on_real_frames`, with
`GREYCARD_MODELS`): the portrait's woman decided alone; her Lips
pasted onto her other frame decided, onto the group asked; the
group's woman pasted onto the portrait asked "someone else"; a turn
and +0.7 EV keep the pick; Whole person on the portrait is hers and
everyone's (19.35% of the frame); on the group each of five is decided
alone, holds her own face, and no two share a pixel, and All people
holds all of each. About 1.6 s for an iris on the GPU, 3.8 s for a
whole person on the group.

Open, on the roadmap: a "use this person on all N pictures" after a
paste onto many; Whole person's edge from Subject's matte; the solid
fill for the other whole-route parts. Face recognition across outfits
stays there too (§253).
