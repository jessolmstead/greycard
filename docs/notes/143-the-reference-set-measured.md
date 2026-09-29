# 143. The reference set, measured (2026-09-22)

Seven of §85's frames came back with Lightroom exports, one slider at
its limit per export and everything else at the default, lens
corrections on in both: the watch (R6 II), the ferry (GFX 100S II), a
sunset over a city with 9 percent of the raw clipped (R5 II), the
lighthouse at +1 EV in both (R5 II), a wedding couple in a white dress
and a dark suit (R6 II), a backlit portrait on a mountain (R8) and a
dim interior under lamps at ISO 800 (R6 II). greycard's side was
rendered from copies of the raws with a sidecar written per variant,
so no hand on a slider is in it. Everything is linear luminance read
back from the JPEGs, and the slider responses are the median change
in stops within percentile bands of each editor's own picture, which
holds whatever the two editors' geometry does.

**The baseline, by camera.** The median against Lightroom's, in stops:
the watch -0.01, the sunset -0.02, the lighthouse +0.01, the wedding
+0.17, the portrait +0.22, the interior +0.26; the ferry +0.56. The
Canon frames sit within about a tenth of a stop of Lightroom on
average, so §141's 0.8 is right for them, and the GFX is the outlier;
the flowers, also GFX, are +0.85 over their camera's JPEG with no
Lightroom export to say more. The camera JPEGs are the worse target:
they scatter about ±0.8 stops from Lightroom (the R8's is a full stop
over both editors), which reads as the camera's scene-adaptive
rendering. What Lightroom does per camera is Adobe's `BaselineExposure`,
the value its DNG Converter writes for each body; that is the table a
per-camera baseline wants.

**The top never reaches white.** On every frame greycard's 99th
percentile sits under Lightroom's: 0.55 against 0.60, 0.93 against
0.99, 0.96 against 1.00, 0.74 against 0.86, 0.76 against 0.88, 0.69
against 0.78, 0.70 against 0.84. greycard clips nothing anywhere;
Lightroom puts 20 percent of the sunset at white and the camera 24,
where greycard renders the sky round the sun as a flat pale grey.
That is §85's shoulder, now with a number from every frame.

**Highlights is stronger than Lightroom's, and narrower.** At -2
against -100, in the bands from the 70th percentile up: the wedding
-1.12 to -1.24 against -0.41 to -0.77, the portrait -1.24 to -1.34
against -0.31 to -0.62, the lighthouse -1.22 to -1.34 against -0.33
to -0.55, the sunset -0.37 to -1.04 against -0.06 to -0.42. Only the
low-contrast watch had greycard's the weaker, which is where the
roadmap's "does nothing" came from. Under the median greycard's does
nothing and Lightroom's takes a gentle -0.1 to -0.2. Lightroom's also
adapts to the frame: on the watch it was strongest in the bright
mid-tones, on the sunset it leaves the clip alone. So greycard's -1 is
roughly Lightroom's -100 at the top, and the ramp wants to reach under
mid grey, lighter.

**Shadows is close.** Within about 0.2 stops on the wedding, the
portrait and the lighthouse. On the low-key interior greycard's lifts
the 40th to 70th percentiles by three stops where Lightroom's lifts
them by 1.3 to 2.3, and gives the darkest tenth less (+2.3 against
+4.0): the face comes up brighter and flatter.

**Whites works on the mid-tones in Lightroom.** On the sunset
Lightroom's -100 and +100 move the 25th to 85th percentiles most (up to
-0.33 and +0.51) and leave the clip where it is; greycard's moves only
the top thirty percent, and at +2 stretches a hard-edged, hueless
white round the sun where Lightroom keeps a warm glow at the edge.

**Blacks: the crush matches, the lift fogs.** On the interior,
greycard's -0.3 against Lightroom's -100 is band for band the same,
-1.5 to -6.9 stops. The lift is not: +0.3 raises the darkest tenth by
+7.4 stops where +100 raises it by +2.3, and the picture goes under a
grey veil. The black point is an offset in scene light, a fraction of
mid grey added to everything and scaled back at scene white; at +0.3
that is 0.054 of scene light, 1.7 stops under grey, laid over a
picture whose darkest tenth sits eight stops down. Lightroom's lift is
a toe: +1.3 to +1.6 stops across the 10th to 70th percentiles of that
dark frame, fading to nothing by the 90th.
