# §293. The grading's sliders follow the chosen wheel (2026-10-10)

The user found the Color grading section's Hue and Strength sliders
stuck on the last wheel they had dragged: enlarging another wheel, or
clicking its name, left the sliders on the old one, so the only way to fine-tune a wheel
was to drag it first and lose the point it had.

The cause was that one thing chose the sliders' wheel: a press or a drag
on a wheel (`grade-press`/`grade-move`), which also set its point from
the pointer. A click on a wheel's name, which enlarges or shrinks it, set
only `grade-big` and redrew; it never touched `grade-range`. And the
chosen wheel was marked only by its name a shade brighter than the
others, which is hard to see.

What we changed:

- A press in a wheel, chosen or not, chooses it and puts its point at
  the pointer, as a press always did, and its new values go onto the
  sliders. We tried a first press that only chose, to spare the point,
  and a wheel that swallows its first click feels dead.
- A click on a wheel's name, enlarging or shrinking it, chooses it as
  well and moves nothing in it, since nothing in it was touched: this
  is the way to fine-tune a wheel's point as it stands
  (`grade-chose`, which replaces `grade-resized`; choosing redraws the
  wheels, which is what put an enlarged one at its size).
- The chosen wheel wears a ring in the accent just outside its rim, and
  its name is in the accent at the medium weight. The enlarged wheel carries the
  ring too when it is the chosen one; a press on a small wheel while
  another is enlarged moves the ring and leaves the enlarged one its
  size.
- The history names the wheel: "Color grading shadows 40°" when only
  its hue moved, "Color grading shadows 25%" when only its strength
  did, "Color grading shadows" when both did, "Color grading balance
  -20" for the balance, and "Color grading" still when more than one
  thing moved. A mask's rows carry its name before it, as every
  other section's do.

The Masks tab's grading is the same section on the same panel
properties, so a mask's wheels behave the same way; the tests drive
both.
