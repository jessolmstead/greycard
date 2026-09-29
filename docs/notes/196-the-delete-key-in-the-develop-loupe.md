# 196. The Delete key in the develop loupe (2026-09-27)

§190 kept the Delete key to the grid and culling. In use, pressing it
over a frame in the develop loupe and getting nothing read as a bug,
so the key now opens the same sheet there too. Nothing in the develop
view binds Delete (no mask, repair or guide is removed with it), a
text field with the focus still has the key first, and the key still
only opens the sheet: nothing goes until it is confirmed. The sheet's
other guards stand: no repeat, no modifiers, no other sheet up.
