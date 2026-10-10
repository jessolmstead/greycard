# 279. Whole person's edge from Subject's matte, as built (2026-10-10)

§263 left Whole person reading fuzzy beside the parts: it comes off SAM 3's
whole-picture pass at 288 cells and is made solid, so its soft band ran
1.3 to 1.6 times Subject's and the loose hair against the sky that Subject
keeps was lost. §263's plan is now built: inside the picked person's region
grown three cells, the edge is Subject's matte, and where someone else is,
SAM's person split still draws the line.

## The composition (`sam3::subject_edge`)

The person's region is what it was: SAM's `person` instance (or their share
of a merged one), filled by §263's rule and ramped solid (`solid`), then
refined at the preview's size as before (`sams`, SAM's edge alone). Beside
it we keep the cells where SAM's edge must stand whatever Subject says
(`keep`): every other `person` instance, filled the same way so a
neighbor's glasses are theirs; the rest of a body SAM merged over two
faces; every other face; and the enclosed gaps the fill left open. Then,
on signed distances to SAM's outline in cells, carried to the preview's
pixels bilinearly so nothing steps at a cell's edge:

- out to `GROW` = 3 cells past the outline, Subject's matte, fading to
  nothing over the last cell; nothing beyond;
- inside, SAM's solid region under it as a floor, from nothing at the
  outline to all of it two cells in, so a blotch Subject leaves in a body
  is filled and at the edge Subject alone says where the person ends;
- within a cell of `keep`, or as near `keep` as to the person, SAM's edge
  alone, blending into the above over the next two cells.

Three cells is 21 pixels at 2048. SAM's cut runs within a cell or two of
the true edge, and the wisps Subject keeps reach a cell or two past it;
three takes them and stops before a neighbor's shoulder or a held thing
Subject also finds salient becomes this person's.

The floor ramp is ours. The build's brief asked for the most of the two
inside the ungrown region; that literal max gave the same bands to within
0.4 on every frame, but it steps from Subject's value to SAM's at SAM's
outline wherever Subject's edge lies inside SAM's, and the ramp leaves no
such line.

A head SAM found with no body (the person is their face mask or its box)
keeps SAM's edge: grown by Subject's matte it took a collar of neck and
shoulders three cells under the chin, cut off there, which is not the head
alone §263 gives such a person.

Three guards came out of the trial frames:

- **Nearer, not only near.** The first build let a gap between a couple
  that Subject fills (it holds them both) go to both: 21 shared pixels
  became 2034. A pixel now takes Subject's matte only when it is nearer
  this person than anything kept, so a gap goes to neither by the matte
  and keeps the person split's values.
- **Specks.** SAM gave the woman of the couple a one-cell speck inside the
  man's glasses, a hole in his raw instance; grown by Subject's matte it
  became a solid blob in his face. A part of the region (four-connected)
  now keeps SAM's edge when it has no cell two in, when most of the cells
  about it are kept, or when Subject's matte holds less than 80 percent of
  its inside (someone Subject does not find salient).
- **Others filled.** A neighbor's instance is filled by the same unsure
  rule before it is kept, so their glasses are theirs.

All people is the same with the union as the region and only the open gaps
kept.

## In the editor

`Ai::part` settles the person first, and only then, for Whole person, asks
`person_edge` for the matte, unturned as the People view is: the one a
Subject or Background made on this develop, in memory (`subject_matte`) or
on disk (Subject's own cached raster); else it runs Subject on the
preview exactly as a Subject shape does and keeps the matte as theirs, so a
Subject, a Background and a Whole person on one picture cost the model one
run. An ask never runs Subject. The Subject model is lent with the People
model to a set's other frames.

Without a Subject model in the store the People run goes ahead with SAM's
edge, and the status line says "whole person found on WebGPU in 0.39 s,
with a coarser edge: the Subject model is not downloaded" (or "Subject did
not run (...)"); the mask still shows. Asking for a Whole person from the
People menu (a person picked, the one person, or All people) offers the
Subject model on the usual sheet beside it, once, the file a Subject would
be offered, unless either file is in or it was declined or failed this
session. A Whole person loaded from a sidecar or pasted never offers it;
it is made with SAM's edge and the status line says why. Declined, the
edge offer is not made again this session ("Whole person keeps its coarser
edge without the Subject model"), while a Subject shape is still offered
its own. While the first Whole person of a session loads Subject, the
status line says so. When the model arrives, Whole person rasters are
dropped on both sides and made again. Their disk key carries `person-edge-1` and whether Subject
was in the store; a raster made while Subject was in but failed to run is
kept in memory only.

The other whole-route parts (Top, Hair, ...) are unchanged. The
composition does not fall out for them: Subject's matte is the whole
person, so in a Hair's grown band it would take the forehead and the
shoulders as hair. Hair against the sky is where it would matter most; it
wants a matte of the hair alone, or Subject's matte cut by the hair's
neighbors (face, skin, top) as `keep`.

## Measured

`whole_person_takes_subjects_edge_on_the_frames` (greycard-ai, ignored,
`GREYCARD_SAM3_FRAMES` and `GREYCARD_MODELS`) on §256's thirteen frames,
WebGPU, each mask drawn into the editor's 2048-wide raster. The band is
the pixels between 0.1 and 0.9 for each edge pixel. Our band runs a
little higher than §263's figures on some frames (we count the whole
raster, lavender and all); before and after are measured the same way.

| Frame | Band before | After | Subject | IoU with Subject, before | After |
|---|---|---|---|---|---|
| portrait, sky behind | 19.0 | 14.3 | 14.8 | 0.993 | 0.999 |
| her other frame, lavender in front | 54.8 | 29.4 | 19.6 | 0.767 | 0.804 |
| portrait in a field | 23.5 | 15.0 | 18.2 | 0.883 | 0.955 |
| studio portrait | 23.3 | 11.1 | 10.9 | 0.951 | 0.975 |
| standing, greenhouse | 17.0 | 12.0 | 11.5 | 0.990 | 0.998 |
| close portrait | 31.2 | 23.2 | 23.5 | 0.989 | 1.000 |
| full length, night street | 18.8 | 16.2 | 16.3 | 0.991 | 1.000 |
| full length, wall | 17.0 | 11.4 | 12.7 | 0.824 | 0.857 |
| small figure, two frames | 15.3, 16.0 | 13.1, 12.9 | 13.2, 12.9 | 0.983, 0.992 | 1.000, 1.000 |
| couple by the sea: all, him, her | 19.0, 20.1, 21.7 | 12.6, 13.9, 15.1 | 11.8 | | |
| couple on a sofa: all, each | 24.6, 32.3, 18.3 | 16.1, 18.3, 14.0 | 14.2 | | |
| group of five: all | 15.1 | 12.4 | 12.6 | 0.949 | 0.981 |
| group of five: each | 14.1 to 15.9 | 11.6 to 13.2 | 12.6 | | |

The widest band fell from 54.8 to 29.4. On the single-person frames and
for All people on every frame but the lavender one, Whole person now sits
within a tenth of Subject's own band, some under it. The individuals of the
couples sit further off (13.9 and 15.1 against 11.8; 18.3 and 14.0 against
14.2), though that is not like for like: Subject's band is everyone's, and
where two people touch the person split keeps SAM's edge on purpose. On the
lavender frame the band halved; the IoU rose only to 0.80 because Whole
person still rightly leaves out the lavender in front of her.

What lies within three cells of the outline and is no one's, a held thing,
the lavender, a neighbor SAM did not find, comes in as a stub of fixed
width ending in a cut line where the grown region stops, never as the whole
object.

No two people share a pixel over a half that SAM's edges did not: the
couple by the sea 21 before and after, the sofa couple and all ten pairs of the group 0 before and after.
Over 0.1 the soft tails touch a little more (37,384 to 38,641 on the
couple by the sea).

In the editor's own path (`parts_on_real_frames`, release, WebGPU): on the
portrait Whole person's band 14.7 against Subject's 14.8, and the Subject
shape asked after it reads the kept matte (0 s); on the group each
person's band 11.6 to 13.2 against Subject's 12.6, and no two of the five
share a pixel over a half, as with SAM's edge alone.

Time, release, WebGPU: the first Whole person of a session pays Subject's
load, 0.59 s to 4.23 s on the portrait. After that, the first Whole
person on a picture pays Subject's run, 0.35 s to 0.63 s on the group;
each further one pays the composing, 0.35 s to 0.39 to 0.46 s. A picture
that already has its Subject matte pays the composing only.

## Tests

- `subject_gives_a_whole_person_its_edge_within_their_region`: by hand at
  two pixels a cell, hair two cells out kept, a held thing six out not, a
  blotch filled, an open gap kept open though Subject closes it, a stray
  arm Subject does not hold keeping SAM's edge, SAM's split to the pixel
  into a neighbor, the neighbor sharing nothing new, and an empty matte
  giving SAM's edge unchanged.
- `cells_to_counts_cells_to_the_nearest_on`: the chamfer distance.
- `a_head_alone_keeps_sams_edge_beside_subjects_matte`: a head with no
  body keeps SAM's edge to the pixel while the one with a body beside it
  takes Subject's; no collar under the chin.
- `a_whole_person_runs_at_once_and_offers_subject_beside_it`: the offer.
- `a_declined_edge_offer_is_not_made_again_and_says_so`: the decline,
  and the menu's Whole person (not another part) marking the offer due.
- `parts_on_real_frames` asserts the band within 1.15 times Subject's on
  the portrait and on each of the group's five (SAM's alone ran to 1.26
  times), the Subject shape reading the kept matte, and no pair in the
  group sharing more over a half than with SAM's edge alone (made beside
  them with a test switch); without Subject in the store it checks the
  status line.

## Left

- Hair and the other whole-route parts, above.
- Wisps that reach past three cells are cut, faded over the last cell;
  on the portrait against the sky a few at her shoulder end there.
- A held thing, foreground (the lavender) or a neighbor SAM did not find,
  within three cells of the outline, comes in as a fixed-width stub
  ending in a cut line.
- The 80 percent cover and the floor depth come from these frames only.
