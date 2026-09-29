# 7. Frontend

### What RapidRAW does per preview frame

GPU render → copy to staging buffer → map to CPU → mozjpeg encode → Tauri IPC
bytes → JS `Blob` → object URL → webview JPEG decode → GPU upload for
compositing. Two GPU round trips, one lossy encode and decode per slider
move. During a drag you see a lossy 8-bit JPEG in the webview's idea of sRGB,
never the actual render. The ROI and reduced-quality interactive path is
effort spent fighting the architecture.

### Native surface design

Engine renders into a wgpu texture, Rgba16Float, linear working space. The UI
draws it as a quad with a small sampling shader: zoom and pan as a UV offset,
then the output transfer function and the monitor ICC (3D LUT). Histogram and
waveform are compute passes on the same texture; only kilobytes come back.
Requirement: engine and UI share one wgpu device and queue.

### Why leave Tauri (for this product)

- Webviews assume sRGB; WebKitGTK on Linux has no usable path to wide-gamut or
  HDR output. The single most important surface would be the one you can't
  manage.
- Platform fragility (the NVIDIA/Wayland crash class).
- Two languages and a JSON serialization boundary; the untyped adjustment blob
  is downstream of it.
- Memory.
- What Tauri buys (web ecosystem iteration speed, CSS polish, i18n tooling,
  cheap Android) isn't what makes an editor trusted. The real loss is months of
  UI polish, not correctness.
- The transparent-webview-over-wgpu hybrid is unreliable on Linux.

### Toolkit assessment (the hard, unresolved question)

The widget set is narrower than it looks: canvas, curves editor and histogram
are custom wgpu drawing in any toolkit. The toolkit's job is sliders, panels,
lists, text, icons. Polish there is a design-system problem solved once.

- **Slint:** the only one whose purpose is product-grade UI. QML-like markup,
  hot reload, Skia text, animations, live previewer, Figma import. Custom
  interactive widgets awkward in a declarative language (but those live in the
  canvas). wgpu texture import shipped recently behind an unstable feature
  flag pinned to a wgpu version; verify on NVIDIA + Wayland with the Skia
  backend. License: GPL-3 or a free royalty-free license with attribution; a
  GPL app takes the GPL side.
- **egui (eframe, wgpu backend):** best wgpu story, fastest iteration; looks
  like a debug tool without a design layer. Rerun's viewer proves it can look
  professional; their design layer is open source and borrowable. Register the
  engine texture as a native texture or use a paint callback. Accessibility
  partial.
- **iced:** shader widget with device/queue/target access; clean model; we've
  fought its styling before and wouldn't again.
- **GPUI:** excellent text/perf, but not wgpu on Linux (blade), sparse docs.
- **Makepad:** built for designed/animated UIs, own GPU backends, texture
  sharing is a project. **Xilem/Vello:** right long-term direction, not ready.
- **GPUI (Zed):** Apache-2.0, superb text and frame times, Tailwind-style
  styling in Rust, `gpui-component` supplies a usable widget set. Wrong fit
  here: on Linux it renders through blade (its own Vulkan layer), not wgpu, and
  it has no custom-render-pass hook, so the engine texture would have to be
  display-transformed engine-side to 8-bit sRGB and uploaded as an image every
  frame. That gives up the managed-output surface the native design exists for.
  Docs are Zed's source; API still moves.
- **Qt 6 via cxx-qt (KDAB, MIT/Apache):** the mature product-grade option.
  Qt Quick can be told to use an existing Vulkan device
  (`QQuickGraphicsDevice::fromDeviceObjects`) and wrap a native VkImage as a
  scene-graph texture, and wgpu-hal can build a device from raw Vulkan handles,
  so real texture sharing is supported at the API level rather than a hack.
  Proper text, accessibility, i18n, HiDPI, color-space plumbing, and recent
  releases have started on HDR output (verify on Linux/Wayland). Krita, digiKam
  and Resolve show it does imaging UI. LGPL-3 dynamic linking is fine for a
  GPL-3 app; Flatpak's KDE runtime ships it. Costs: a C++ toolchain and CMake
  in the build, QML as a second language (typed properties/signals, not JSON),
  and a large learning surface. Slint is essentially Qt Quick redone in Rust
  by ex-Qt people, which is why it comes first; Qt is the fallback if the
  Slint spike fails, ahead of Flutter.
- **Flutter shell + engine over FFI:** the fallback only if both Rust spikes
  fail. Looks excellent; Linux native texture path goes through GL, and it's
  Dart.

### Plan

Two one-week spikes, same brief: three-panel layout, a slider panel bound to a
state struct, a thumbnail strip, a wgpu texture drawn in the middle with zoom.
One in Slint, one in egui starting from Rerun's design layer. Judge on: does
the texture import work on the NVIDIA Wayland machine; how long did the slider
panel take to look finished; how painful was a custom widget.

Before either: one week on design tokens (type scale, spacing scale, dark
palette with proper contrast, vector icon set such as Lucide or Phosphor, the
four states of every control). What makes Rust UIs look amateur is skipping
this step.

---
