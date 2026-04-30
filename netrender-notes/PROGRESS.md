# WebRender wgpu Notes Index

This file is the short index for the surviving `wr-wgpu-notes/` set.

## Active Branch (2026-04-28)

The `idiomatic-wgpu-pipeline` branch (off `upstream/upstream`) is the
active development branch. The `spirv-shader-pipeline` branch is dead
state — no new work lands there. See
[`2026-04-28_idiomatic_wgsl_pipeline_plan.md`](2026-04-28_idiomatic_wgsl_pipeline_plan.md)
for the active plan.

## Current Canonical Notes

- `2026-04-28_idiomatic_wgsl_pipeline_plan.md`
  - active plan: jump-ship to a clean wgpu-native fork of
    `upstream/upstream`; authored WGSL only; no GL backend; no
    SPIR-V intermediate; no artifact pipeline

- `2026-04-27_dual_servo_parity_plan.md` _(superseded 2026-04-28)_
  - goal: SPIR-V-derived GL and wgpu backends at full parity with original
    0.68 GL; fork drives both upstream servo/servo (GL) and servo-wgpu (wgpu);
    three tracks: GL parity, wgpu coverage, dual-servo compatibility gate

- `2026-04-18_spirv_shader_pipeline_plan.md` _(superseded 2026-04-28)_
  - authored-SPIR-V target architecture and branch direction
- `2026-04-21_spirv_pipeline_reset_execution.md` _(superseded 2026-04-28)_
  - concrete reset slice and branch-state execution note
- `2026-04-08_live_full_reftest_confirmation.md`
  - latest direct full local reftest confirmation
- `archive/progress/2026-04-10_p15_progress_report.md`
  - wgpu 29 bump, inter-stage vars root cause, headless reftest mode
- `servo_wgpu_integration.md`
  - downstream Servo integration notes and host/device-sharing shape
- `2026-04-18_upstream_cherry_pick_plan.md` _(superseded 2026-04-28)_
  - upstream-integration strategy of record: selective cherry-pick from
    `upstream/upstream` onto this branch, batch ordering, working-method recipe
- `2026-04-22_upstream_cherry_pick_reevaluation.md` _(superseded 2026-04-28)_
  - per-candidate accept / defer / reject watchlist with dated ancestry checks;
    read alongside the cherry-pick plan when triaging upstream picks

## Active Follow-up Plans

- `2026-03-01_webrender_wgpu_renderer_implementation_plan.md`
  - broader renderer convergence history; no longer canonical for the shader pipeline reset
- `2026-04-26_track3_legacy_assembly_isolation_lane.md` _(superseded 2026-04-28)_
  - concrete slice ladder for Phase 5 closure Track 3 (runtime legacy source assembly isolation)
- `draw_context_plan.md`
- `typed_pipeline_metadata_plan.md`
- `texture_cache_cleanup_plan.md`
- `wasm-portability-checklist.md`

## Archive

- `archive/progress/`
  - dated progress snapshots kept for historical traceability
- `archive/legacy/`
  - older branch-shape notes, debug plans, and diagnostics writeups that are
    no longer the primary source of truth
  - includes the archived GLSL-front-end translation journal

## Local Logs

The `logs/` directory is intentionally local-only and gitignored except for its
`.gitignore` file. Keep only artifacts that still support an active note or an
unresolved diagnostic thread.
