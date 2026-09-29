# 2. Why an engine and not a patch

An editor whose stated goal is a fast, clean path to a creative look
will not take a pipeline-wide rewrite for colorimetric accuracy, and
should not be expected to: it is a different product for a different
photographer, and that is a legitimate position rather than a defect.

Color correctness is also not a thing that can be added at the edges.
It constrains the decode, the buffer, the matrix, the working space and
every consumer of the base image at once (§3, §4). Anything that applies
white balance in the working space, or picks a matrix without reference
to the scene illuminant, bakes in an error no slider removes later. That
is an engine-level decision, so the engine is where the work goes.

---
