# 70. The denoiser's weights published (2026-09-18)

The three tiers are at `huggingface.co/jessolmstead/greycard-denoise`,
the address the registry has carried since §37: `denoise-v19.onnx`,
`denoise-v15.onnx` and `denoise-v20.onnx` at the repo's root on `main`,
byte for byte the files under `tools/denoise/runs/`, so no hash or size
in `registry.rs` changed. The repo is public under GPL-3.0 with a model
card: the tiers, the contract (`packed` 1×4×h×w in the stabilized space
in, `rgb` 1×3×2h×2w out), a training summary and links back here.

Pushed with the `hf` CLI (`huggingface_hub` in a venv at `~/.venvs/hf`,
logged in as the project's account) rather than the web form, so the
next push is one command. Checked three ways: the public URLs hash to
the registry's values; a new ignored test,
`the_store_fetches_every_denoiser_tier` (`GREYCARD_MODELS` at an empty
directory and `GREYCARD_FETCH=1`), fetched all three in 2.4 s and found
the GPL note beside each; and `greycard develop --ai-denoise fast` with
`XDG_CACHE_HOME` at that empty cache loaded v19 on WebGPU and wrote its
answer to the denoise cache. The last open item before a first tag was
this one; what remains for 0.0.1 is a version in the workspace, a
README that lists every crate and what gets fetched on first use, and
a release note.
