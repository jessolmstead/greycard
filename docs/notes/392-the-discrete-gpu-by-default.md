# §392. The discrete GPU by default, and the clipping toggles' on state

*2026-10-22*

A tester's laptop (issue #35) has an AMD iGPU beside an RTX 3050, and
greycard ran on the iGPU: Slint asks wgpu for an adapter with the power
preference `WGPU_POWER_PREF` names and none where it names nothing, and
wgpu under no preference takes the first adapter the loader lists. On a
hybrid laptop that is the integrated one. The iGPU then lost its device
after the first develop, which is a separate matter (§393 if it gets
one), but the editor should not have been there to begin with.

## The change

The window asks for high performance where the variable names nothing.
That is one field on Slint's `WGPUSettings`, set beside the limits in
`panel/startup.rs`; the software retry reuses the same settings. A value
set by hand is honored, so `WGPU_POWER_PREF=low` still puts the window
on the iGPU, and the log says which preference was asked for and where
it came from. greycard-gpu's own instances already asked for high
performance; greycard-ai's adapter probe now applies the same rule, so
on one machine the three agree.

wgpu drops adapters that cannot present to the window's surface before
it sorts by preference, so a discrete card that cannot drive the screen
is never chosen. A desktop already on its one card sees no change. An
Intel Mac or a hybrid laptop now wakes the discrete card, which costs
battery; that is the point, and the variable is the way back.

A first version set the variable itself before Slint's select, with a
marker variable and a scrub for child commands, the way the software
adapter and the interface scale are handled. The reviewer pointed out
that only Slint's default reads the variable, so the field is enough
and the `set_var` goes.

## The clipping toggles

The two buttons on the RGB scope that show the shadow and highlight
clipping marks in the picture showed "on" as a border one shade lighter
on an 8 px dot (issue #37). On, the control is now filled in the accent,
as an on switch is, with the dot outlined in the canvas color so a lit
one still reads on the fill.
