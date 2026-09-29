# 57. The picture that stays while the next one develops (2026-09-18)

Choosing a file from the strip swapped the panel to the new file's
edit at once, and the viewport reads the panel every frame, so for the
two seconds of the decode and the develop the old picture sat under
the new file's look: its exposure, its curves, its masks, its crop.
Found in use and put in the roadmap's bug list.

The viewport now keeps a copy of the outgoing edit (`State::held`)
when a file is chosen while a picture is on the GPU, and draws under
that in place of the panel's until the new file's develop lands, when
it opens fitted as before. It sits above the history panel's peek in
the same choice. The white balance preview is identity meanwhile, as
the open frame and its profile are already the next file's and the
picture was developed at its own white point. A develop that fails
drops the hold, so the old picture shows the panel's look as it did,
rather than freezing. The camera's own preview as a stand-in was the
other option; holding the old picture cost a field and five lines and
needs no second path through the shader.
