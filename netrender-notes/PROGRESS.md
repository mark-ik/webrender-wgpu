# netrender notes index

This file is the short index to the surviving plans in
`netrender-notes/`. The repo is post-cleanup: vello is the sole
rasterizer on `main`, the upstream WebRender codebase (`webrender_api/`,
`wrench/`, `wr_glyph_rasterizer/`, etc.) has been removed and lives on
the `webrender-wgpu-upstream/` side worktree if anyone needs to spelunk
through the original implementation.

## Current canonical plans

These two are the source of truth for the live architecture.

- [`2026-04-30_netrender_design_plan.md`](2026-04-30_netrender_design_plan.md)
  — the parent plan: phases 0.5 → 12, axioms, crate split rationale,
  Scene API contract, render-task graph, tile cache, axiom 14
  compositor seam. Most of phases 1–9 landed; 10/11/12 still pending
  per the vello plan's §12 phase mapping.

- [`2026-05-01_vello_rasterizer_plan.md`](2026-05-01_vello_rasterizer_plan.md)
  — the vello pivot, runtime-verified through phase 7'. Replaces the
  parent plan's batched-WGSL rasterizer. Status block at the top
  records which §11 spike outcomes cleared. Phase 7' (Masonry pattern
  tile cache) is the architectural heart and is delivered.

  **Post-pivot findings landed since (2026-05-04):** persistent
  per-frame image cache (§11.9 wart, see also §11.16 polish), op-list
  refactor with consumer-push painter order (§11.11),
  variable-radius box-shadow blur via cascaded passes (§11.10),
  `FontBlob` unified to `peniko::Blob<u8>` (§11.9), nested layers +
  arbitrary-path clips via `SceneOp::PushLayer/PopLayer` (§11.14),
  hit testing — stack-returning, layer-clip-aware, per-glyph
  approximate (§11.12, §11.15, §11.16), `netrender_text` parley
  adapter with decoration painting (§4.4 status block, §11.16),
  edition-2021 bump, `Scene::clear_ops` helper. Test count: 105
  passing, 1 ignored, 0 failed across the workspace.

  Open items split between two documents (§11.99 was folded out
  for findability):

  - [`2026-05-04_feature_roadmap.md`](2026-05-04_feature_roadmap.md)
    — Phase R (open refinements / wart fixes) + Phases A–F (new
    capability, diagnostics first).
  - [`2026-05-05_deferred_phases.md`](2026-05-05_deferred_phases.md)
    — three architecturally-significant deferrals (12c' backdrop
    filter, 13' compositor handoff, linear-light blending), each
    with its own short design plan.

## Active follow-up plans (small scope)

- [`2026-05-04_feature_roadmap.md`](2026-05-04_feature_roadmap.md)
  — Phase R (open refinements / wart fixes — was §11.99 of the
  rasterizer plan) + Phases A–F (new capability: diagnostics
  first, then consumer-pull-imminent, then SceneOp expansions,
  then architecturally-significant).
- [`2026-05-05_deferred_phases.md`](2026-05-05_deferred_phases.md)
  — the three architecturally-significant deferrals from the
  rasterizer plan (12c' backdrop filter, 13' compositor handoff,
  linear-light blending), each with trigger, work shape, and
  alternatives.
- [`draw_context_plan.md`](draw_context_plan.md)
- [`typed_pipeline_metadata_plan.md`](typed_pipeline_metadata_plan.md)
- [`texture_cache_cleanup_plan.md`](texture_cache_cleanup_plan.md)
- [`wasm-portability-checklist.md`](wasm-portability-checklist.md)
- [`servo_wgpu_integration.md`](servo_wgpu_integration.md)
  — downstream Servo integration notes; host/device-sharing shape

## Historical / superseded

The plans below predate the vello pivot and the move to this fork.
They describe approaches that are no longer the path forward — kept
for context, not for guidance. **Do not treat as active.**

- `2026-04-28_idiomatic_wgsl_pipeline_plan.md` — the
  idiomatic-wgpu-pipeline branch's approach (authored WGSL only, no GL,
  no SPIR-V intermediate). Was the active plan before the vello pivot;
  preserved on its own branch (`idiomatic-wgpu-pipeline`).
- `2026-04-28_renderer_body_wgpu_adapter_plan.md` — `WgpuDevice`
  adapter early-stage planning. Subsumed by netrender_device's
  current shape.
- `2026-04-29_pipeline_first_migration_plan.md` — typed-pipeline
  migration, batch-builder discussion. Pre-cleanup.
- `2026-04-30_phase_d_rollback_to_skeleton.md` — record of the rollback
  that preceded the netrender split.
- `2026-04-30_servo_wgpu_integration_assessment.md` — pre-fork servo-
  integration assessment.
- `2026-04-08_live_full_reftest_confirmation.md` — last GL/wrench
  reftest confirmation before the fork.
- `2026-04-18_spirv_shader_pipeline_plan.md` — dead direction.
- `2026-04-18_upstream_cherry_pick_plan.md` — superseded by the fork.
- `2026-04-21_spirv_pipeline_reset_execution.md` — superseded.
- `2026-04-22_upstream_cherry_pick_reevaluation.md` — superseded.
- `2026-04-24_tile_with_spacing_validation_error.md` — historical bug
  diagnostic.
- `2026-04-26_track3_legacy_assembly_isolation_lane.md` — superseded.
- `2026-04-27_dual_servo_parity_plan.md` — superseded by the fork.
- `2026-04-28_session_brief.md` — historical session note.
- `2026-03-01_webrender_wgpu_renderer_implementation_plan.md` — the
  original convergence history, no longer canonical.

## Local-only

- `archive/` — dated progress snapshots and older branch-shape notes,
  kept for historical traceability.
- `logs/` — local-only, gitignored except for its `.gitignore`. Only
  retain artifacts supporting an active note or unresolved diagnostic.
