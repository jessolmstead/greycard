# The learned highlight fill's oracle

The inference path of the shipped learned highlight fill (r6e-s1 run with
e_tied's look), cut from the highlight-learn trial with its numerics
unchanged. It is the reference the Rust port is tested against, and it
writes the fixtures under `crates/greycard-core/tests/fixtures/highlight_fill/`.
`SPEC.md` is what the port works from: every stage's arithmetic, the
tolerances, the coverage, and where hl-1 departs from the trial.

- `oracle.py`: the fill. `fill(model, x, codes, clips)` runs the whole
  frame; `box`, `frame`, and `thumb` give the trial's crop instead; `f`
  is a reduced develop's look scale. Every constant is at its top.
- `scenes.py`: the synthetic scenes (formulas only, no photograph).
- `checkpoint.py`: the fixtures' random-seeded checkpoint and the weights
  format (`manifest.txt` plus one raw little-endian f32 file a tensor).
- `make_fixtures.py`: writes the fixtures; its docstring lists each array.
- `export_weights.py`: a checkpoint to the single weights blob greycard
  builds in, and the check arrays its test compares against.
- `run_blob.py`: the oracle with a built-in blob's weights on fixture
  scenes, the expected fills of greycard-ai's end-to-end test.
- `compare_trial.py`: the oracle against the trial's own code, the whole
  frame against the trial's box, and float32 against float64.

## Regenerate the fixtures

Needs numpy and torch (CPU). From the repository root:

```sh
CUDA_VISIBLE_DEVICES="" python -I -B scripts/highlight-fill-oracle/make_fixtures.py \
    crates/greycard-core/tests/fixtures/highlight_fill
```

The output is the same bytes run to run (torch 2.14.1, numpy 2.5.2, four
threads). No trained weights and nothing from a photograph go into it.

## Check against the trial

```sh
CUDA_VISIBLE_DEVICES="" python -I -B scripts/highlight-fill-oracle/compare_trial.py \
    TRIAL/scripts/highlight-learn [CHECKPOINT.pt]
```
