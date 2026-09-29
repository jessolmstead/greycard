# 184. The tab bar pinned (2026-09-27)

The Develop, Crop, Masks and Retouch tab bar moved out of the panel's
scroll view into a fixed row of its own, under the scopes header and
above the sections. This reverses §147, which left the tabs scrolling
with the sections on the reasoning that pinning them would cost a
laptop-height panel another row. A tester asked for the bar to stay
put, and the row it costs measures about 36 px, so the trade is
accepted now. The gap under the bar is the scroll content's own top
padding, as before, so the first section sits where it did; the
review caught a doubled gap in the first cut and the two builds were
laid over each other to confirm the fix.

Because the tab row is no longer inside the scrolled content, a given
`--panel-scroll` offset (still the scroll view's `content-y`) reaches
about 36 px further down than it did before: a snapshot at some PX
now shows roughly what PX + 36 showed. The offsets quoted in earlier
sections, §147's 600 px among them, are measurements against the
layout of their day and are left as they were.
