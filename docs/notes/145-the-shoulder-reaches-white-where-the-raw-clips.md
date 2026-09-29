# 145. The shoulder reaches white where the raw clips (2026-09-22)

§85 left it open and §143 put a number on it from every frame:
Narkowicz's fit of the ACES output transform reaches display white
5.3 stops over mid grey, and nothing a sensor records gets there. Scene
white, where a channel the raw clipped lands, sits 2.47 stops over
grey (§19), and 3.27 with §141's baseline, where the fit gives 0.897.
So the sky round the sun in the sunset frame, 9 percent of the raw
clipped, rendered as a flat pale grey where the camera's JPEG puts 24
percent of the picture at white and Lightroom 20, and every frame's
top sat under Lightroom's.

**Reaching white without moving the rest.** `tone` is the fit,
untouched up to mid grey; over it a gain eases in by a smoothstep in
stops, from one at mid grey to 1.114 at 3.27 stops, the gain that
takes the fit there to exactly one, and past that the curve is one.
Both factors rise, so the curve does, and a test sweeps it. It meets
white with the fit's own slope there, about a tenth of a display stop
per scene stop, so the clip is a soft corner rather than a hard one.
The mid-tones do not move: a stop over grey takes +3.5 percent, and
everything at or under grey is the fit's to the bit, so §141's
baseline and §143's medians stand. The white is named against scene
white plus the baseline because that is where the sensor's clip is in
the curve's terms: an exposure pulled down takes the clip under white
again, as it does in Lightroom, and the highlights and whites sliders
act before the curve, so they still recover what the curve would clip.
The viewport's `tone` is the same arithmetic; the fit view against the
export scaled to it differs by 1.86 of 255, +0.04 signed, on the
sunset, whose detail is most of that.

**Measured after.** The sunset puts 17.3 percent at white against
Lightroom's 20.1, its 90th percentile at 0.995 against 0.992, and in
the picture the sky round the sun is white. The frames whose top is not
the sensor's clip come up but stay a little under Lightroom's, their
99th percentiles now 0.742 against 0.844 (the lighthouse), 0.820
against 0.880 (the wedding), 0.737 against 0.775 (the portrait), 0.805
against 0.862 (the interior) and 0.573 against 0.597 (the watch), from
0.695, 0.755, 0.687, 0.744 and 0.553. What is left is the shape of
Adobe's curve between mid grey and white, steeper than the fit's, which
is the same question as the highlights and whites reshaping and is
left for it.
