/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `netrender_text` — `parley::Layout` → `netrender::Scene` adapter.
//!
//! Bridges parley's shaped/laid-out text into a netrender Scene as
//! `SceneGlyphRun`s. The adapter is one-way: parley owns shaping,
//! line-breaking, BiDi reordering, font fallback, and alignment;
//! netrender owns GPU rasterization.
//!
//! ## Design boundary
//!
//! The contract between text-layout and render is a **data type**:
//! `netrender::SceneGlyphRun`. Any shaper that produces SceneGlyphRuns
//! is interchangeable. This crate is *the parley adapter* — if a
//! consumer wants `cosmic-text` instead, they write
//! `netrender_cosmic_text` that emits SceneGlyphRuns the same way;
//! nothing in `netrender` or `netrender_text` needs to change.
//!
//! There is deliberately no `Shaper` trait on the netrender side —
//! see the rasterizer plan §2.2's "abstraction without users"
//! reasoning, which applies identically to text.
//!
//! ## Brush type
//!
//! The brush is fixed to `[f32; 4]` (premultiplied RGBA, matching
//! netrender's color contract). To vary color across runs in the
//! same layout, push styled spans with different brushes via parley's
//! builder before laying out:
//!
//! ```ignore
//! let mut builder = layout_cx.ranged_builder(&mut font_cx, text, 1.0, true);
//! builder.push_default(StyleProperty::Brush([1.0, 1.0, 1.0, 1.0]));
//! builder.push(StyleProperty::Brush([1.0, 0.0, 0.0, 1.0]), 5..10);
//! let layout = builder.build(text);
//! ```
//!
//! ## Font registration
//!
//! Within a single `push_layout` call, fonts are deduped: the same
//! `parley::FontData` referenced by multiple runs registers only
//! once with the scene. Across calls, fonts re-register — if you
//! call `push_layout` repeatedly on the same scene with the same
//! font, `scene.fonts` accumulates duplicates. A persistent
//! cross-call font map is a future addition; for one-frame consumers
//! it's irrelevant, and for streaming consumers a per-frame Scene
//! rebuild is the typical pattern.
//!
//! ## Decorations (underline / strikethrough)
//!
//! Both are painted as filled rects spanning the glyph run's
//! horizontal advance, with thickness and offset taken from
//! parley's `RunMetrics` (font-supplied) or the per-style override
//! when set. Painting order follows the CSS text-decoration spec:
//! underline → glyphs → strikethrough, so the strikethrough
//! crosses through the glyphs and the underline sits cleanly below
//! their descenders.
//!
//! ## What this crate does NOT do
//!
//! - Inline boxes (`PositionedLayoutItem::InlineBox`). Skipped — the
//!   consumer placed those, the consumer renders them.
//! - Synthesis (synthetic bold/italic). parley's `run.synthesis()`
//!   returns hints; netrender's glyph pipeline currently doesn't
//!   honour them. Use real bold/italic font files instead.

use netrender::{FontBlob, FontRegistry, Glyph, Scene};
use parley::{Layout, PositionedLayoutItem};

/// Push every glyph run from `layout` into `scene`, positioning the
/// layout's top-left at `origin` (device-pixel coordinates).
///
/// Convenience wrapper that builds a fresh [`FontRegistry`] per
/// call. For consumers that build many layouts into one Scene per
/// frame (or share fonts across consumers under a C-architecture
/// shared master Scene), use [`push_layout_with_registry`] with a
/// persistent registry — that gives cross-call font dedup so
/// `scene.fonts` doesn't grow by N per N `push_layout` calls.
pub fn push_layout(scene: &mut Scene, layout: &Layout<[f32; 4]>, origin: [f32; 2]) {
    let mut registry = FontRegistry::new();
    push_layout_with_registry(scene, &mut registry, layout, origin);
}

/// Push every glyph run from `layout` into `scene`, positioning the
/// layout's top-left at `origin` (device-pixel coordinates).
/// Reuses `registry` for font dedup across calls.
///
/// Fonts referenced by the layout register into `scene.fonts` via
/// `registry`: the same `parley::FontData` (matching the
/// `(Blob::id(), font-collection index)` pair) registers exactly
/// once per registry lifetime, regardless of how many
/// `push_layout_with_registry` calls reference it.
///
/// Runs are emitted in parley's iteration order: top-to-bottom by
/// line, left-to-right within a line (after BiDi reordering).
pub fn push_layout_with_registry(
    scene: &mut Scene,
    registry: &mut FontRegistry,
    layout: &Layout<[f32; 4]>,
    origin: [f32; 2],
) {
    for line in layout.lines() {
        for item in line.items() {
            let glyph_run = match item {
                PositionedLayoutItem::GlyphRun(gr) => gr,
                // Inline boxes are placed by the consumer (image,
                // shape, nested layout). Not our problem to render.
                PositionedLayoutItem::InlineBox(_) => continue,
            };

            let run = glyph_run.run();
            let font_data = run.font();
            // parley's FontData and netrender's FontBlob both hold a
            // `peniko::Blob<u8>`; clone is an Arc bump plus id copy,
            // no bytes copied. The registry dedups by (Blob::id(),
            // index) so the same font interns once across calls.
            let font_id = registry.intern(
                scene,
                FontBlob {
                    data: font_data.data.clone(),
                    index: font_data.index,
                },
            );

            let style = glyph_run.style();
            let color = style.brush;
            let font_size = run.font_size();
            let metrics = run.metrics();

            // Run extents in scene-space. parley's `offset()` is
            // the x-position of the first glyph along the line
            // baseline; `advance()` is the run's total horizontal
            // advance. `baseline()` is the y-position of the
            // baseline within the layout.
            let run_x0 = origin[0] + glyph_run.offset();
            let run_x1 = run_x0 + glyph_run.advance();
            let baseline_y = origin[1] + glyph_run.baseline();

            // Underline rect — painted before the glyphs so the
            // glyphs draw over it (CSS text-decoration spec).
            if let Some(underline) = &style.underline {
                let offset = underline.offset.unwrap_or(metrics.underline_offset);
                let thickness = underline.size.unwrap_or(metrics.underline_size);
                let y_top = baseline_y - offset;
                let y_bot = y_top + thickness;
                scene.push_rect(run_x0, y_top, run_x1, y_bot, underline.brush);
            }

            let glyphs: Vec<Glyph> = glyph_run
                .positioned_glyphs()
                .map(|g| Glyph {
                    id: g.id,
                    x: origin[0] + g.x,
                    y: origin[1] + g.y,
                })
                .collect();

            if !glyphs.is_empty() {
                scene.push_glyph_run(font_id, font_size, glyphs, color);
            }

            // Strikethrough rect — painted after the glyphs so it
            // crosses through them.
            if let Some(strikethrough) = &style.strikethrough {
                let offset = strikethrough.offset.unwrap_or(metrics.strikethrough_offset);
                let thickness = strikethrough.size.unwrap_or(metrics.strikethrough_size);
                let y_top = baseline_y - offset;
                let y_bot = y_top + thickness;
                scene.push_rect(run_x0, y_top, run_x1, y_bot, strikethrough.brush);
            }
        }
    }
}

/// Re-export parley so consumers can build layouts without taking a
/// direct dependency on a possibly-different parley version. Use as
/// `netrender_text::parley::{FontContext, LayoutContext, …}`.
pub use parley;
