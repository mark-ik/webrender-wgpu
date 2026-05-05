# netrender — deferred phases (2026-05-05, activated 2026-05-05)

Originally a tracker for three architecturally significant items
held back from the active roadmap. All three were activated the
same day this doc was opened. This file is now the **handoff
record** — what each was, where it went, why.

The items:

- **12c' backdrop filter** — now active; see roadmap [`D1` in
  `2026-05-04_feature_roadmap.md`](2026-05-04_feature_roadmap.md#L225-L233).
  Implementation approach: extend `SceneLayer` with an optional
  `backdrop_filter: Option<SceneFilter>` field; slice the
  per-frame op list at each backdrop-filter index and run a
  multi-pass dance (render below → filter → composite) using the
  existing render-graph filter pipeline (`brush_blur` from 11c').
  No conflict with 13' below — backdrop is a within-render
  multi-pass; compositor handoff is a post-render blit.

- **13' native-compositor handoff (axiom 14)** — now active via
  **path (b′)**. See
  [`2026-05-05_compositor_handoff_path_b_prime.md`](2026-05-05_compositor_handoff_path_b_prime.md)
  for the full design. Path-summary: keep the single master
  render (Masonry preserved), expose dirty-region info per
  declared compositor surface at the API level, hand the master
  texture to the consumer's `Compositor::present_frame` which
  owns the per-dirty-surface blit into its native textures.
  Distinct from the doc's original (a)/(b) dichotomy: (b)'s
  dismissal was wrong — damage info isn't lost, it just wasn't
  exported. Consumer for the trait: servo-wgpu, which is
  reshaping its compositing layer to consume `Compositor` from
  netrender_device.

- **Linear-light blending** — still upstream-blocked, but no
  longer passively watched. Active monitoring via a
  `linear-light-canary` cargo feature that re-runs `p1prime_03`
  against the current vello dep on every CI bump. Canary greens
  → trigger fired → ~50-line wrap to expose `BlendSpace` /
  `interpolation_color_space`. Canary setup folds into roadmap
  Phase A (diagnostics-first) as a new entry; the eventual wrap
  lives in Phase R (wart fix scope, upstream-gated).
  See roadmap for the canary entry.

---

## Why this doc collapsed

The original framing was "deferrals gated on consumer pull at the
project-direction level or upstream-blocked." Three pieces moved
the items from deferred to active in one step:

1. **12c' was double-tracked.** The roadmap already had `D1` for
   the same item. Consolidating onto `D1` removed the duplication.
2. **13' has a real consumer.** servo-wgpu is in this workspace
   and willing to reshape its compositing layer. The
   project-direction trigger fired.
3. **Linear-light's "no active monitoring" framing was the
   actual bug.** A CI canary turns it from passive into
   automatically-triggered without writing any of the eventual
   wrap code prematurely.

The category "architecturally significant deferrals" turned out
to contain three different shapes (project-direction-gated,
upstream-blocked, mis-categorized duplicate) — none of which
shared a load-bearing property. With all three activated, the
category itself dissolves.

## When to revive this file

Add a section here when a new architectural deferral surfaces
that doesn't fit the roadmap's Phase R / Phase A-F shape — i.e.,
something bigger than a wart fix that we're consciously parking
rather than activating. If it's just "not yet, but on the
roadmap," it lives on the roadmap, not here.
