# §453. A named lens the database lacks falls to the body

*2026-10-29.* §413 found a compact's lens from its body when the file
named none, and left one case: a file that names its lens as a focal
range, "5.0-70.0 mm f/3.1-5.9", as CHDK's DNGs do, matched nothing in
lensfun and got nothing, with "no profile for 5.0-70.0 mm f/3.1-5.9"
in the panel (a PowerShot SX220 HS on pixls). The body's lens is now
tried whenever the named one finds no match, not only when none is
named; a fixed-lens body has no other lens to be wearing. An
interchangeable body naming an unknown lens still gets none, since
its mount is not a fixed one. The profile carries whether the lens
came from the body, so the editor's log and the command line say so
either way.
