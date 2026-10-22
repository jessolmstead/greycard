# The scopes read the export, not the monitor's table (2026-10-21)

§42 promised that the histogram, the scopes, and the clipping lamps
are of the export space's encoded values. §349 found they were not:
the analysis picture (§38) went through the display table, so a
monitor profile moved every bin, the readout under the pointer, and
the lamps. So did a P3 or Rec.2020 export on an sRGB monitor. Even
the identity table was not quite the identity: a GPU's filtered
lookup holds its weights in as few as 8 bits, and on the test field
5168 of 523776 bins drawn through it differed from the same picture
drawn bare.

**The fix.** The analysis draw stops at the encoded output: the
shader's spare `cubic.w`, set by a draw `Through::Nothing`, skips
the table's lookup and the proof's gamut mark. The viewport keeps
the table; its own clipping paint already read the value before it.

**The navigator** (§44) showed the analysis picture and is promised
as what the viewport shows. So the analysis draws twice, 512 wide:
once bare, binned and kept for the readout, and once through the
table and the proof for the navigator. Both land in one staging
buffer under one map, so the read back's bookkeeping and the lost
map's recovery are as they were. The cost is one more 512-wide draw
when the edit changes, at most once per read back.

**Measured.** `the_scopes_read_the_export_not_the_monitors_table`
renders the test field bare, through an Adobe RGB monitor's table,
and through that table with an sRGB proof's gamut marks: every
scope's bins and the readout at four points are equal across the
three, while the navigator moves 172,649 of 174,592 pixels and
equals the viewport drawn at its size. With the analysis drawn
through the table again, the first RGB bin holds 514 against 19,360.
In the editor under headless weston, a 45 MP frame with a wide-gamut
display profile against `--no-display-profile`: before, the
histograms differed in 2,724 of 20,480 pixels and the shadow lamp
lit only without the profile; after, the histograms are identical
and the lamp lights in both.
