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

## Active follow-up plans (small scope)

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
