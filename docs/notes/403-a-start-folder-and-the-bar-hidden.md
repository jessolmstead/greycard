# §403. A start folder, and the filter bar hidden

*2026-10-23*

Two of a tester's asks (pixls, 2026-10-22), as two rows at the top
of the Settings sheet's Library category.

## Start in

greycard reopened the last folder. Now a two-way choice: Last
folder, as before, or a chosen folder picked through the native
chooser, its path shown under the row. At a launch with no path on
the command line the chosen folder's frames are tried first, then
the last folder's, then the chooser opens as it always did for
nothing; a chosen folder that is missing, or that exists and holds
no raws, falls back with one log line and nothing on screen. The
reviewer named the case: the likely use is a card-dump folder
emptied after each import, and a chooser at every launch would be
a nag. The resolution is a pure function over a listing passed in,
so a fake listing tests it. A path is taken as written, with no
expansion of `~`; the chooser is the only way in.

## Show the filter bar

A switch, on by default. Off, the bar leaves both the grid and the
cull strip and its height goes back to the frames. The filters are
the state's and keep applying unseen, so the grid can show fewer
frames than the folder holds with only the header's count as the
hint; the guide says so. Sort lived on the bar and would have gone
with it, leaving no way to change the order, so its button moved
into its own component and sits on the grid header's top row while
the bar is off; culling has no row to hold it and leans on the
grid's. The filter key does nothing while the bar is hidden, logged,
since a pending focus request would otherwise grab the field the
moment the bar came back.

## Not done

- An active filter shows no sign while the bar is hidden beyond the
  header's count.
- On Windows the chosen path reads in environment-variable form,
  as About's folders do.
