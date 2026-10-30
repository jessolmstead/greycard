# §456. The way back from the export queue

*2026-10-29.* A tester viewed the export queue, pressed Loupe at the
top, and found a frame with no tools and nothing saying why
(discussion 51). Not a defect: the queue's Loupe is frozen on
purpose (§416, §430), the queued frame under the edit that will
export, with the tools hidden since changing it there would make
the export disagree with its tile. The gap was that neither button
at the top left led out of the queue's view: Loupe only closed the
grid, so the queued frame came up frozen, and Library only opened
the queue's grid again. The only way out was another listing in the
left pane, which the same tester had asked for as a way back.

Now the queue remembers what was open before it, the view, its
folder when every file shares one, and the frame as a version key so
a virtual copy comes back as itself; the two buttons at the top
leave for that, the folder read again from disk so a file removed
or written meanwhile shows as it is, the frame selected once that
read lands and the Loupe shown only then, so the queued frame never
flashes first. A read overtaken by another never shows it. When the
queue was the first view, or the view before cannot be reopened,
they go to an empty folder view, never a frozen frame. Opening a
queued tile from the queue's grid, by a double-click, Return, G, E,
or D, still gives the frozen Loupe, and that Loupe now says so where
the tab bar would be: "Queued for export, shown as it will export.
Edit it from its folder." The one cost is that from a frozen Loupe
the way back to the queue's grid is G or Esc, since Library now
leaves.
