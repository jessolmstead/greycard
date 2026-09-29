# 147. The scopes stay at the top of the panel (2026-09-22)

The file's name, the shot's three lines and the scopes moved out of
the panel's ScrollView into a layout of their own above it, so the
sections scroll under them and the histogram stays in sight of a
slider far down the Develop tab. The tabs stay with the sections:
pinned too, they would take another row from a panel that is short
of height on a laptop, and they are one scroll away.
`panel-scroll` is still the ScrollView's `content-y`, so
`--panel-scroll` measures from the tabs now rather than from the
file's name. A snapshot at 600 px shows the header unmoved and
Exposure under it.
