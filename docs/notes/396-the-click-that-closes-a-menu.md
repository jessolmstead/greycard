# §396. The click that closes a menu

*2026-10-23*

A tester found that dismissing the frame's context menu with a click
on the picture recentered the picture there: the click zoomed. The
cause is how Slint delivers a press outside a menu, which is not what
the code around it assumed (and what §208's comment said).

In the pinned Slint, a press outside a menu opened with close-on-
click-outside is sent to the ContextMenuArea's parent subtree only,
never to what is under the pointer; the menu closes after. Our frame
menu is a zero-sized item at the window's corner, so that press
reaches nothing. Its release has no grab and goes to whatever is under
the pointer, and a TouchArea fires `pointer-event` Up whether or not
it saw the press. By then the menu's close had given focus back, so
the viewport's guard was off, and a release with no press read as a
click, and so a zoom.

The fix is at the receiver: the picture's TouchArea keeps a `held`
flag set on its own press and cleared on release and cancel, and a
release without a press is ignored. A real click always delivers its
press, so nothing real is swallowed. The level and guide strokes end
only if one began, and read a cancel ahead of the button filter,
since a cancel carries no button. The curve editor and the tint ring
get the same `held`, since the ring's release ran a view change on a
lone release. The grid was never affected: it acts on `clicked`,
which needs the press.

The existing test for this clicked inside the menu and so picked its
first item, proving nothing; it now clicks clear of the menu and
fails without the fix.
