# 255. The strip's scroll bar, and Home and End (2026-10-06)

Two small items for 0.5.0, built together because they touch the same
keys and the same scrolling. The film strip scrolls by wheel and keys
since §89, but it showed neither where it was in the folder nor a
handle to drag there. And Home and End did nothing in the grid or the
strip.

**One bar, either way.** §214's grid bar becomes `ScrollBand` in
`controls.slint`, a component that lies down the right edge or along
the foot. The grid's is the same bar as before, and its captures
match master's pixel for pixel. The strip's runs along its foot over
the same arithmetic (`bar-length`, `bar-offset`, `bar-drag`, the band's
length standing for the sheet's height). The offset stays the
owner's: the bar asks to scroll through `scroll-to`, and the owner
holds that to its range as it holds the wheel's. It behaves as the
grid's does:

- faded at rest and full strength while scrolling and a moment after;
- a thin thumb that widens on a lit track under the pointer;
- a press in the track pages, and the thumb drags;
- either wheel over it moves the strip along;
- a press that closes the frame menu is nothing more.

The timer that fades it runs only while it is awake, so a strip at
rest asks for no frame. One frame shows no bar.

**The strip's bar sits in its own band.** The strip's padding under
the cells is 12 px (`strip-pad`), and the bar lives there rather than
over the frames. So its track never covers a frame's ring, and a frame
keeps every pixel of its click. A first version laid the bar over the
frames' foot: the ring was cut, and a click on a frame's last row went
to the bar. The track's corner radius is the smaller of 7 px and half
the band, so the grid keeps its 7 px.

**Home and End.** In the grid and the strip, Home and End choose the
first and last frame, as the arrows choose, and scroll there even when
that frame is already chosen. Under a filter they go to the first and
last frame shown. Shift+Home and Shift+End take the run from the
current frame to that end, as Shift+click does: the run is the shown
frames only, and a current frame hidden by the filter gives the target
alone, as a Shift+click from a hidden anchor does. In the filter
field, Home and End are the field's.

**Not from this change.** The reviewer's click run crashed at exit in
NVIDIA's driver, a worker thread in `wgpu::Device::poll` while the
editor shut down with a develop still running. The same crash is in
the core dumps from master builds of 2026-10-03, so it predates this
change. It is not §248's crash, which was in the Vulkan loader at
instance creation.
