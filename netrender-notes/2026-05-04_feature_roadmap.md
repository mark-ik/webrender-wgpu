# netrender — feature roadmap checklist (2026-05-04, last updated 2026-05-05)

Forward-looking work items in two layers:

- **Open refinements** (Phase R) — known wart fixes on shipped
  features. Each is a specific, bounded change with a documented
  design; gated on consumer pull. Originally lived as §11.99 of
  the rasterizer plan; folded in here so all open items are in
  one place. Move entries into a `§11.x — CLEARED` finding in
  [`2026-05-01_vello_rasterizer_plan.md`](2026-05-01_vello_rasterizer_plan.md)
  when they land.
- **New capability** (Phases A–F) — features the codebase doesn't
  yet express. Ordered for "build the measuring stick before more
  features": diagnostics first, then consumer-pull-imminent, then
  SceneOp expansions, then architecturally-significant.

Activation history of the originally-deferred items
(12c' backdrop filter, 13' compositor handoff, linear-light
blending) lives in
[`2026-05-05_deferred_phases.md`](2026-05-05_deferred_phases.md).
All three were activated 2026-05-05; their canonical entries are
on this roadmap (D1, D3, A5+R9 respectively). The 13' design
detail lives in
[`2026-05-05_compositor_handoff_path_b_prime.md`](2026-05-05_compositor_handoff_path_b_prime.md).

Each entry has a brief, an estimated scope, a trigger / dependency
note, and a receipt sketch — enough to pick up without re-deriving
the design.

---

## Phase R — Open refinements (consumer-pull-gated wart fixes)

Each entry says what the wart is, when it bites, and what the fix
shape is. Move into a `§11.x — CLEARED` finding in the rasterizer
plan when landed.

- [ ] **R1. Real font-metric per-glyph hit testing**
  (rasterizer plan §11.15).
  *Bites when:* a consumer needs precise click-on-character
  behaviour (text editors, selection caret placement). Today's
  em-box approximation is enough for "click on this label" UI
  but imprecise around descenders and kerning gaps.
  *Shape:* use `skrifa::metrics::GlyphMetrics` (parley already
  pulls skrifa transitively) to get real glyph bounds at the
  run's font_size. ~50 lines.

- [ ] **R2. Per-segment point-in-polygon for `SceneOp::Shape`**
  (rasterizer plan §11.12).
  *Bites when:* a consumer hit-tests an arbitrary path
  (hexagonal node, custom widget shape) and the AABB hit area
  is too sloppy.
  *Shape:* use `kurbo::Shape::contains` after building the
  `BezPath` from the `ScenePath`. ~20 lines.

- [ ] **R3. Layer-clip path-precise containment**
  (rasterizer plan §11.16).
  *Bites when:* a layer has an arbitrary `SceneClip::Path`,
  and a consumer needs hit-testing to honor the path's true
  shape (not its AABB). Today rounded corners and arbitrary
  path interiors are AABB-conservative — points near corners
  or outside the path-but-inside-the-AABB still register hits.
  *Shape:* same `kurbo::Shape::contains` machinery as R2,
  applied to the layer-clip stack pre-pass.

- [ ] **R4. Image cache for the simple (non-tile) rasterizer**
  (rasterizer plan §11.9-ish).
  *Bites when:* a consumer uses the simple `scene_to_vello`
  path (non-tile) on image-heavy scenes. The simple path's
  `build_image_cache` rebuilds peniko ImageData blobs every
  call because it has no state to hold them in.
  *Shape:* a stateful wrapper struct (mirror of
  `VelloTileRasterizer::image_data`) or move the cache up into
  the caller. ~40 lines.

- [ ] **R5. Downscale-blur-upscale for very large blurs**
  (rasterizer plan §11.10).
  *Bites when:* a consumer needs `blur_radius_px > ~28`.
  Today's multi-pass cascade caps at `MAX_PASSES = 50`, which
  σ-clips beyond that radius. Skia and Firefox use downscale-
  then-blur for large radii — same approach would lift the cap.
  *Shape:* render to a half-or-quarter-resolution intermediate
  texture, blur there, upscale to target. Two more render-graph
  tasks per "downscale level."

- [ ] **R6. Inline-box rendering helper in `netrender_text`**
  (rasterizer plan §4.4).
  *Bites when:* a consumer's text contains inline images /
  nested layouts (`<img>` tags, embedded widgets). The adapter
  currently skips `PositionedLayoutItem::InlineBox`; consumer
  renders them themselves.
  *Shape:* not a netrender concern in the long run — inline
  boxes are placed by the consumer (because their content is
  consumer-typed). What's missing is a per-line layout helper
  that walks parley's items in order and lets the consumer
  render boxes inline with the text.

- [ ] **R7. CSS font-cascade rules** (rasterizer plan §4.4).
  *Bites when:* a consumer's content needs DirectWrite/
  CoreText-style font selection (CJK fallback chains,
  `font-family` priority lists with locale-aware preferences).
  Fontique enumerates system fonts but doesn't implement the
  full cascade.
  *Shape:* not a netrender concern — embedders that need this
  implement it on top of fontique or wrap a system text engine.
  Listed here for visibility, not implementation.

- [ ] **R8. Synthetic bold/italic via parley `Synthesis`**
  (`netrender_text` doc).
  *Bites when:* a consumer requests a font weight/style the
  system doesn't have a real face for. Parley's `Synthesis`
  flags are ignored. Use real bold/italic font files instead.

- [ ] **R9. Linear-light blending wrap** (rasterizer plan §6.3,
  Pitfall #2; `p1prime_03`).
  *Bites when:* upstream vello's GPU compute path honors
  `peniko::Gradient::interpolation_cs` — until then, gradient
  interpolation and `mix-blend-mode: linear-light` only match CSS
  reference under the `vello_hybrid` path, which we don't use.
  *Trigger:* the **A5** canary greens.
  *Shape:* ~50 lines — add `Scene::interpolation_color_space`
  (default `Srgb` for back-compat), thread through to
  `peniko::Gradient::with_interpolation_cs`, drop the
  caveat block in [rasterizer §3.3](2026-05-01_vello_rasterizer_plan.md#L298-L308),
  update §6.3 contract to advertise dual capability.
  Don't pre-build the enum; vello's API shape will dictate it.

---

## Phase A — Diagnostics first

Build the measurement infrastructure before the next round of
features. Every Phase B+ item ships cheaper if these exist: capture
a real consumer scene as a regression artifact, watch the dirty
tiles when adding a primitive, profile the impact when adding a
filter. Order within Phase A is by value-to-cost ratio, smallest
first.

- [ ] **A1. Op-list inspector** — pretty-print `Vec<SceneOp>` to
  a string for debugging. Scope: ~50 lines, zero design questions.
  Receipt: `Scene::dump_ops()` returns a multi-line string with
  per-op summary (kind, key fields, transform/clip if non-default).
  Cheapest item; useful immediately.

- [ ] **A2. Scene capture / replay** — `Scene::snapshot()` →
  serializable record, `Scene::replay(&record)` rebuilds. Scope:
  ~80 lines + serde dep gating. Receipt: capture a frame from a
  real consumer, ship it as a `*.scene.bin` artifact, replay
  deterministically in a unit test. Multiplies the value of every
  other test / regression diag in the rest of this list.

- [ ] **A3. Tile-dirty visualizer** — overlay that paints dirty
  tiles in red on a debug pass. Scope: ~120 lines including the
  per-tile `last_dirty_frame` field on `TileCache`. Receipt: an
  `enable_tile_dirty_overlay: bool` flag on `NetrenderOptions`;
  when on, dirty tiles get a translucent red wash on top of the
  rendered output. Bites first when middlenet performance gets
  weird.

- [ ] **A4. Frame profiler** — per-phase timings: scene build,
  tile invalidate, vello encode, GPU submit, readback. Scope:
  ~150 lines + a profiling dep (`puffin` or thin custom). Receipt:
  `Renderer::last_frame_timings() -> FrameTimings` with named
  spans. Optionally exposes vello's internal `Renderer` timing
  hooks too.

- [ ] **A5. Linear-light canary** — CI canary that re-runs
  `p1prime_03` against the current vello dep on every bump,
  surfacing the moment vello's GPU compute path honors
  `peniko::Gradient::interpolation_cs`. Scope: ~30 lines + a
  CI job. Receipt: a `linear-light-canary` cargo feature gates
  the test; CI runs `cargo test --features linear-light-canary`
  on dep bumps. Greens → trigger fired for **R9** (the eventual
  wrap). Replaces the passive "every vello version bump,
  re-test by hand" cadence the previous deferred-phases doc
  documented.

---

## Phase B — Consumer-pull-imminent

Things smolweb / middlenet will surface as parley wiring stabilizes
and graphshell-shaped consumers wire in.

- [ ] **B1. Selection highlight + caret emission** — the next
  thing middlenet will pull on once it has shaped text. Scope:
  ~100 lines of adapter code. Receipt: `netrender_text` exposes
  `selection_rects(layout, range) -> Vec<[f32; 4]>` (one rect per
  visual line in the selection), and a thin caret helper that
  emits a blink-friendly thin rect at a `parley::Cursor`. Caret
  blink is consumer-side; netrender paints the rect.

- [ ] **B2. Scrolling convenience** — `Scene::push_scroll_frame
  (clip_rect, scroll_offset)` macro that opens a layer with a
  rect clip + a translate transform, with a matching `pop_scroll
  _frame()`. Scope: ~30 lines, no architectural commitment — just
  ergonomics over existing primitives. Receipt: the demo gains a
  scrolling card list under one method call instead of three.

- [ ] **B3. Color emoji / COLR fonts** — vello + skrifa already
  handle COLR layer rendering on the glyph path; we likely get
  this for free. Scope: 0 lines of code, ~30 lines of test.
  Receipt: load an emoji-bearing font (Segoe UI Emoji on Win,
  Apple Color Emoji on Mac, Noto Color Emoji on Linux), shape a
  string with emoji, render — emoji should appear in color, not
  as black silhouettes. If they appear in color: done. If not:
  this becomes a real work item gated on whichever upstream piece
  is missing.

---

## Phase C — Capability unlocks (`SceneOp` territory)

New op variants / extensions that genuinely expand what the API can
express. Each is an additive change to `SceneOp`; the rasterizer
gains one match arm per item.

- [ ] **C1. Stroke decorations** — line caps (`butt` / `round` /
  `square`), joins (`miter` / `round` / `bevel`), dash patterns.
  Scope: ~80 lines. Receipt: `SceneStroke` gains optional
  `cap`, `join`, `dash_pattern` fields; the rasterizer plumbs
  them through to `kurbo::Stroke`. CSS `border-style: dashed`
  becomes expressible.

- [ ] **C2. `SceneOp::Pattern`** — repeated-tile fill (CSS
  `background-image` with `repeat`). Scope: ~120 lines including
  the `SceneOp::Pattern { tile: ImageKey, extent: [f32; 4],
  scale, transform_id, clip_rect, clip_corner_radii }` variant.
  Receipt: tile a 64×64 image across a 256×256 area with one
  push call; without this, consumer pushes 16 copies.

- [ ] **C3. Mask-image fills** — using one image as an alpha
  mask for another fill (any `SceneOp` body). Scope: ~80 lines
  + 1 new WGSL helper or a vello layer trick. Receipt:
  `Scene::push_layer_mask(image_key, ...)` opens a layer whose
  visibility is gated by the mask image's alpha. Decouples mask
  from fill so the consumer doesn't have to pre-bake.

- [ ] **C4. Variable fonts axis interpolation** — parley + skrifa
  support this; the scene needs to thread axis values through to
  vello's `draw_glyphs`. Scope: ~40 lines on the netrender_text
  side + 20 lines through the rasterizer. Receipt: a single font
  rendered at three different `wght` axis values produces three
  visibly distinct weights in one frame.

---

## Phase D — Architecturally significant

Items that need real design conversation, not just implementation.

- [ ] **D1. Backdrop filter** — frosted-glass blur of *what's
  behind* a translucent rect. Distinct from drop shadow.
  Architecturally hard because vello's "always overwrite the
  whole target" model is in tension with reading the backdrop.
  Scope: 200-400 lines + a render-graph integration. Receipt:
  CSS `backdrop-filter: blur(12px)` produces a frosted-glass nav
  bar over a busy background. Pre-design discussion needed:
  do we do snapshot-then-blur-then-composite-over (multi-pass), or
  use a vello layer trick if one exists in newer vello.

- [ ] **D2. Animated values** — interpolate alpha / transform /
  color over time. Scope: 200-500 lines depending on the timing
  model. Receipt: `SceneOp::PushLayer` accepts `Animated<f32>`
  for alpha; the renderer either samples the curve at a given
  time or the consumer rebuilds the scene per frame with
  resolved values. **Design choice:** does netrender own the
  timing (and read a clock), or does the consumer drive per-
  frame and rebuild ops with resolved values? My read: keep
  netrender clockless; provide an `interpolate.rs` module of
  timing curves and let the consumer drive. But this needs a
  conversation before we ship.

- [ ] **D3. Native-compositor handoff (axiom 14) via path (b′)** —
  exporting per-surface textures to native OS compositors so the
  OS applies transform / clip / opacity at 60Hz without re-
  rasterizing. Single vello render submit preserved (Masonry
  intact); per-surface dirty bits surface the damage info; the
  consumer's `Compositor` impl owns the post-render copy pass
  and submits its own encoder.

  **Status (2026-05-05):** netrender-side sub-phases 5.1–5.4
  shipped. Eight receipts at
  [`tests/p13prime_path_b_present_plumbing.rs`](../netrender/tests/p13prime_path_b_present_plumbing.rs)
  cover plumbing, master-pool reuse + resize, dirty-clean-after-
  unchanged, dirty-on-bounds-change, destroy-on-undeclare,
  z-order, blit-dirty-only (with real `copy_texture_to_texture`
  submitted by the test stub Compositor), and transform-only-
  clean. Full netrender suite green (118/118).

  **Remaining:** sub-phase 5.5 — servo-wgpu adapter implementing
  the `Compositor` trait against servo-wgpu's compositing layer,
  reshaping its rendering-context surface to feed
  `render_with_compositor`. Lives in the `servo-wgpu` repo,
  separate workspace. Mark D3 complete and migrate to a
  `§11.x — CLEARED` finding in
  [`2026-05-01_vello_rasterizer_plan.md`](2026-05-01_vello_rasterizer_plan.md)
  when 5.5 lands.

  Full design in
  [`2026-05-05_compositor_handoff_path_b_prime.md`](2026-05-05_compositor_handoff_path_b_prime.md).

---

## Phase E — Performance / scaling

Gated on Phase A4 (frame profiler) data. Don't pre-build; profile
under real consumer load first.

- [ ] **E1. GPU damage tracking** (sub-tile). Today vello re-
  encodes the whole scene every frame. For scrolling content
  where most of the screen is unchanged, this is wasted compute.
  The fix lives at vello's encoder level and is upstream's
  responsibility — see §13 risk #8 (accepted). Track but don't
  start; requires upstream coordination.

- [ ] **E2. Multi-thread scene building** — building
  `Vec<SceneOp>` in parallel chunks and joining. The op-list
  shape is conducive (no cross-references between siblings).
  Scope: ~150 lines including a `SceneFragment` builder type and
  a join API. Receipt: a 4-thread scene build of 10k ops takes
  <2× the wall time of a 1-thread build of 2.5k ops. Real
  consumer pull only — middlenet's scene size has to actually
  show CPU pressure first.

---

## Phase F — Platform / output

Bigger commitments that broaden what platforms / output formats
the renderer reaches.

- [ ] **F1. HDR / wide-gamut output** — Display P3 / Rec2020.
  Scope: untracked; depends on vello's color-pipeline trajectory.
  Receipt: a render target in P3 produces visibly more saturated
  reds/greens/blues vs. sRGB on a P3-capable display. Track
  upstream; this is more "watch and wait" than "build."

- [ ] **F2. WebAssembly target** — browser-hosted netrender.
  Scope: ~50 lines of `boot()` adapter selection + a wasm-bindgen
  shim crate. The wgpu side handles webgpu adapter natively.
  Receipt: `wasm-pack build` produces a `.wasm` that runs the
  demo card grid in a browser canvas. Existing
  `wasm-portability-checklist.md` is the gating list.

---

## When to revisit this list

Add an item when a real consumer surfaces a need that doesn't fit
the current API. Move an item out of the list (into a `§11.x —
CLEARED` plan finding) when it lands. Re-order liberally based on
what the consumer is actually using.

The Phase A items multiply the value of everything below them —
every B/C/D feature is easier to ship, test, and debug if A1-A4 are
already in place. That's the only ordering decision in this file
that's load-bearing; the rest is opportunistic.
