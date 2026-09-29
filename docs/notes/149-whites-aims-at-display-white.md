# 149. Whites aims at display white (2026-09-22)

The white point brings a luminance `whites` stops under its target to
the target, by a power about mid grey. Its target was scene white,
2.47 stops over grey, which was where the raw clips when §85 wrote it.
§141's baseline moved the clip to 3.27 stops in the curve's terms, and
§145 put display white there, so whites was still aiming at a point
that was neither: whites +1 brought the top to 2.47 stops, a pale grey
under the new shoulder, and neither direction lined up with the clip.
It aims at `DISPLAY_WHITE_STOPS` now, on both paths: +1 makes a
luminance a stop under the clip white, -1 puts white a stop further
out and compresses the top 3.27 stops evenly towards grey.

The range holds: the slider's +2 is 1.27 stops short of the pole,
where the exponent is 2.6, and the floor's -3.5 gives an exponent of
0.48, over the 0.41 where the eased curve would fold. On the sunset
-2 now takes the top five percent down 0.17 to 0.22 stops and leaves
0.17 percent at white, and +2 puts 28.2 percent at white against
Lightroom's 27.9; the fit view against the export at +2 differs by
1.99 of 255. Whites' shape, the top thirty percent where Lightroom's
moves the mid-tones, is unchanged and waits with §150's questions.
