# 4. Strategy: fork vs from scratch vs plugin

### Tracking fork: no

The maintainer lands ~260 commits a quarter, many in the shader and processing
module, exactly where the work has to live. The divergence needed is large
(working space, schema, units, every sRGB assumption). The merge tax grows with
divergence; this is how tracking forks die.

### Hard fork: no

Freezing at a release avoids the merge tax but leaves you owning ~100k lines
you didn't write in an architecture chosen for a different goal. You'd rewrite
the core anyway, inside someone else's codebase, without his velocity.

### Plugin: nothing to plug into

RapidRAW has no plugin or scripting surface. Only inbound hooks: it can be
launched as an external editor (`rapidraw --edit <file> --output <file>`) and
there's a headless export mode. White balance sits between decode and
everything else, the worst possible place for a plugin hook; a plugin API that
could host it is a bigger ask than the PR.

### Pre-processor: a real option, and the first product

The DxO PureRAW pattern: a standalone tool reads the RAW, does decode, WB and
camera matrix correctly, and writes a linear DNG any editor opens. RapidRAW
already treats linear RAW as supported. Works with every editor, needs nobody's
permission, is a thin CLI over the engine crate, ships months before an editor
could. Costs: the white point is baked at pre-process time, two-step
workflow, doubled storage, and never better than the host's pipeline after
the DNG lands. A stepping stone, not the destination.

### Recommendation: engine-first from scratch

Not from zero: `rawcolor` + rawler + the shader knowledge already exist. Build
an **engine crate** with no UI dependency, CLI first (the pre-processor), UI as
a separate crate. Nothing about that shape stops another editor consuming the
engine later, the way one can consume `rawcolor` today.

Honest cost: a minimal editor on top of the engine (decode, tone and color
tools, crop, masks, export, library grid) is 6–12 months of focused solo work.
Parity with RapidRAW's breadth is years. Professional viability is about
trust (correct color, predictable, edits that mean the same thing next year),
not breadth. A narrow tool that gets those right is viable.

Start the engine at low intensity now, while the PR sits; let the PR outcome
decide only how much effort keeps flowing upstream.

---
