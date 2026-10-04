# 240. The camera match converges its exposure match, and holds every frame out (2026-10-04)

Built by an opus author and read twice by an opus reviewer, who
found the test that the every-frame holds had broken.

§238 measured why a match under AgX lands worse than one under per
channel, and found the larger cause was the exposure match, not the
curve: one pass reads the camera's distance in stops off the develop's
own exposure and takes a stop on the slider to move the finish a stop.
The display curve does not. It moves the mid-tones by less and the toe
by more, so one pass leaves each frame a different distance from the
camera, and the shared table cannot learn a per-frame miss. Converging
that match took the held-out error from 0.0161 to 0.0131 per channel
and from 0.0195 to 0.0154 under AgX. §238 measured this with the tool.
This section puts it in the match. Built by an opus author.

**One function for the match and the tool.** The secant step moved
from `match_compare` into `camera_match` as `iterate`, with its
constants (`CONVERGED`, `PASSES`, `SLOPE_RANGE`, `MAX_STEP`).
`matched` now does the whole exposure match: the first pass, its
finish, then `iterate` from the match's own two points. It returns
both: the first pass (its offset, the residual it leaves and its
pairs) and where the iteration ended (offset, residual, finishes,
whether a step lost every block) with the pairs the fit takes. The
match keeps the converged pairs. `--match-compare` keeps both, so it
still measures the match as it was and as it runs, on the same
develops, through the same call. A test of `matched` on a real finish
(flat patches, the camera's JPEG made as the develop finished 1.2
stops brighter) shows both ways one pass misses: in the mid-tones it
falls 0.12 stops short per channel and 0.18 under AgX. In the toe it
overshoots by 1.0 and 1.5 stops. Under both curves the iteration lands
within 0.002 stops of 1.2 in three or four finishes.

**We kept §238's step as measured.** The slope is held between −4 and
−0.25, a step is at most a stop, and a frame gets at most eight
finishes. On the real frames below nothing came near those limits (no
frame at the cap, no step that lost its blocks, the largest miss 0.005
stops), and changing a measured step to tidy it would only mean
measuring it again.

**A frame that does not converge is kept, as the tool kept it.** Only
the first pass can drop a frame: if no flat, unclipped block is left
at the match's first offset, the frame is dropped with that reason, as
before. If a later step leaves no block, the iteration stops at the
last offset that had some and the fit takes those pairs. If the eighth
finish has not converged, the fit takes the pairs where it stands.
Either way the frame stays in the fit, and the log says how it ended
("stopped where a step left no block", "stopped at the cap") beside
its offset, residual and finish count. The secant has never reached
either case on the library, so dropping such a frame would be a rule
with nothing to test it against.

**The sheet's error is held out over every frame.** The four-frame
figure was a cheap stand-in (§181) and, per §238, too noisy to size
anything: off the every-frame figure by up to 0.0039, and on one group
it put the gap between the curves at a tenth of its size. `fit_group`
fits the table, takes its fitted ΔE and holds each frame out once. The
holds are spread across the threads, since that is forty fits a group
rather than four. The result line now says "ΔE 0.013 held out over
every frame (0.011 fitted)", `HELD_OUT` is gone, and so is the tool's
"held out (4)" column. The tool's headline now shows each curve's
figures under one pass and converged, and the converged pair is what
the sheet reports.

**Checked on the library.** `--match-compare` on a copy of the index,
narrowed to three Canon groups (R5 Standard, R5 II Faithful, R5 II
Standard): 102 frames developed in 17 minutes under `nice` on eight
threads. The one-pass figures match §238's to the fourth decimal in
every cell. The converged held-out figures match in five of six. The
sixth is R5 Standard under AgX, 0.0130 against §238's 0.0131, which is
expected: §238's iterated column partly used records converted from
the first build's plain step, whose converged offsets were within
0.0054 stops of the secant's. The tool also calls the sheet's own
`fit_group` on the converged blocks now, and it gives the same fitted
and held-out figures as the tool's analysis in every group and curve.
Own blocks, per channel / AgX:

| group | frames | one pass fitted | one pass held out (all) | converged fitted | converged held out (all) | §238 iterated |
|---|---|---|---|---|---|---|
| R5 Standard | 28 / 29 | 0.0056 / 0.0065 | 0.0105 / 0.0133 | 0.0048 / 0.0060 | 0.0081 / 0.0130 | 0.0081 / 0.0131 |
| R5 II Faithful | 22 / 22 | 0.0119 / 0.0142 | 0.0129 / 0.0154 | 0.0115 / 0.0133 | 0.0125 / 0.0145 | 0.0125 / 0.0145 |
| R5 II Standard | 35 / 35 | 0.0125 / 0.0161 | 0.0185 / 0.0248 | 0.0100 / 0.0132 | 0.0145 / 0.0210 | 0.0145 / 0.0210 |

The fitted error comes down with the converged match too (0.0125 to
0.0100 per channel on the R5 II Standard), as it should: frames that
agree on brightness are easier to fit together.

**What it costs.** All 85 and 86 frames converged: 1.99 finishes a
frame per channel and 2.37 under AgX, counting the first pass's own
finish. That is about one more finish a frame than one pass, not the
two §238 gave. §238's mean counted only the frames its plain step had
to redo, which were the far ones. Timed in the same run, under one
curve as the match spends it: the develop and the camera's JPEG 3.98
s, the lay 2.06 s, one finish 0.44 s. So one pass cost 6.5 s a frame
past the develop's start, and the converged match costs 0.43 s more
per channel and 0.64 s more under AgX, or 7 to 10 percent. A group of
forty frames takes about 17 to 26 seconds longer. The user's editor
was exporting through the learned denoiser during part of the run, so
these are a busy machine's seconds. We timed `fit_group` itself (the
fit plus a fit for each frame held out) on eight threads under `nice`:
0.9 to 1.2 s for groups of 22 to 29 frames and 1.9 to 2.2 s for 35, so
about two and a half seconds for forty. The sheet pays that once a
group for the honest figure. The holds run on rayon's global pool, so
the editor's other work queued there, a preview's decode in the
browser, can wait behind them for that long while a group's figures
are made.

**The cost showed in the tests first.** The first version of the
change failed the browser's test that a preview is made off the thread
(`cull::tests::the_local_preview_reads_no_file_and_a_decode_makes_one`)
three times in three; master passed it three times in three. This was
not load. In a debug build `greycard-match` was unoptimized, so each
camera match test's `run_with` put 20 to 40 slow `Model::fit` holds on
the global pool. The cull test's `rayon::spawn` waited in the queue
behind them past its ten-second deadline, and the editor's test binary
went from 28 s to 87 s. The workspace now builds `greycard-match` at
`opt-level = 3` in the dev profile too
(`[profile.dev.package.greycard-match]`). With that the binary's suite
takes 32 to 34 s and the cull test passed in both full runs, and the
match crate's own tests take about 2 to 6 s.

**Not done.** The per-frame contrast term §238 suggests (the frame
matched at two quantiles, ceiling 0.0131 and 0.0146) and the black cut
that costs AgX a quarter of its deep shadows are still open. The sheet
shows no progress during the holds, about two and a half seconds after
a group's last frame, and the holds do not have a pool of their own.