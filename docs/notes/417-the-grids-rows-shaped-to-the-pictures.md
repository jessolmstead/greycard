# §417. The grid's rows shaped to the pictures

*2026-10-24*

A square cell fits a 3:2 picture with 56 px of plate above and below it
at the 176 cell, so a row of landscapes sat 202 px from the next. A
cell now keeps its width, so the columns, the zoom steps, and the
column arithmetic are untouched, and a row is as tall as the tallest
picture in it at that width: 120 px of picture for a row of 3:2, a
pitch of 146 where it was 202. Measured on a folder of mostly
landscapes at 176: rows 146 apart, a fourth row on screen where three
were; on a mixed folder most rows hold a portrait and keep the 202.

Row-max rather than a true justified layout, where each row's frames
are scaled to fill the width: the cells stay a fixed width, so the
columns, the arrows by row, the folder tiles above, and the strip all
keep their arithmetic, and a frame's place does not move when a
neighbor's shape arrives. A portrait is held to the square it had: its
thumbnail is made at the cell's long edge, and a taller room would
stretch it. A row with one portrait among landscapes is tall; that is
accepted, as Lightroom does the same.

The pictures sit at the top of their row, not centered: the tops line
up and the row reads as a row, with a landscape's name under its own
picture and the space below it the sheet's. The ring, the badges, and
the name keep their places on the picture, since a cell is the picture
and its name, not the row.

The index has no frame dimensions, so a frame's shape is its
thumbnail's, turned as its cell turns it; before one comes, 3:2. Shapes
that arrive are laid out once a turn of the event loop, not once a
picture, and the scroll moves by however far the first row on screen
moved, so pictures coming in above the screen do not shift what is on
it. The layout is a prefix sum of row tops, made once per change and
kept: 0.9 ms for 20 000 frames in a debug build. The headings of the
export queue are row breaks in the same table. The lightbox keeps
square tiles.

When shapes arrive, the scroll is corrected in one of three ways,
tried in order. If the sheet was at its end and the selected frame
was on screen, the frame is revealed from the new end, so a row that
grew still shows whole and the sheet stays as near its end as that
allows; if the sheet was at its end with no selection on screen, it
goes to the new end. Otherwise a selected frame on screen is held in
place and revealed again. Failing both, the first row on screen holds
its place. The first draft held only the first row, and a frame
revealed at the foot of the screen, or reached with End, dropped
below it as the portraits above arrived; the review's probe found it
and a test now walks the three cases.
