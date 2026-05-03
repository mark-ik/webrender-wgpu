/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 10a.2 — pure-Rust glyph rasterization wrapper around
//! [`swash::scale::ScaleContext`].
//!
//! Per axiom 16 (external resources are local by the time they hit
//! the renderer), rasterization is fundamentally a consumer concern.
//! This module ships as a public convenience for the common-case
//! consumer that wants the canonical Rust outline + bitmap
//! rasterizer without having to wire it themselves; consumers with
//! their own rasterization stack (Parley, vello scene-as-atlas,
//! swash with custom hinting policy) just don't use it.
//!
//! Note (2026-05-02): swash 0.2.7 internally pulls
//! [`skrifa`](https://docs.rs/skrifa) — the Linebender font crate
//! that the design plan flagged as a future migration target. The
//! plan's "swap rasterizer behind a stable interface when Linebender
//! ships a skrifa-native rasterizer" is partly already resolved
//! upstream.

use crate::scene::GlyphRaster;

/// Source priority used by [`RasterContext::rasterize`]. Outline first
/// (most TTF / OTF fonts ship vector glyphs); monochrome bitmap
/// strikes second (Proggy and other EBDT fonts); color bitmap strikes
/// third (Phase 10b will introduce a parallel color-aware path —
/// today the color bitmap is squashed into its alpha plane).
const SOURCE_PRIORITY: [swash::scale::Source; 3] = [
    swash::scale::Source::Outline,
    swash::scale::Source::Bitmap(swash::scale::StrikeWith::BestFit),
    swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
];

/// Reusable rasterizer state. One per consumer thread; the `swash`
/// internals cache scaled outlines and other shape data inside the
/// scale context, so reusing one [`RasterContext`] across many
/// [`rasterize`](Self::rasterize) calls is faster than building a
/// fresh one per glyph.
pub struct RasterContext {
    inner: swash::scale::ScaleContext,
}

impl RasterContext {
    pub fn new() -> Self {
        Self { inner: swash::scale::ScaleContext::new() }
    }

    /// Rasterize one glyph at `px_size` from `font_bytes` (TTF / OTF /
    /// collection) at the given `font_index` (`0` for single-font
    /// files; per-face for `.ttc` / `.otc` collections). Returns
    /// `None` if the font fails to parse, the glyph is missing, or
    /// rendering fails.
    ///
    /// Sources are tried in order:
    ///
    /// 1. `Source::Outline` — vector glyphs (most TTF / OTF fonts).
    ///    `hint` enables TrueType hinting against the requested
    ///    pixel grid; recommended for small sizes.
    /// 2. `Source::Bitmap(BestFit)` — monochrome embedded bitmap
    ///    strikes (EBDT). Picks the closest-fit strike size.
    ///    Bitmap-only fonts (Proggy) hit this path.
    /// 3. `Source::ColorBitmap(BestFit)` — color emoji bitmap
    ///    strikes (CBDT). Forced into the alpha format below;
    ///    Phase 10b will introduce a parallel color-aware path.
    ///
    /// The output is always single-channel `R8` coverage (`zeno`
    /// `Format::Alpha`). Color emoji currently degrades to its
    /// alpha plane; preserving color requires the dedicated atlas
    /// + shader sub-task in Phase 10b.
    pub fn rasterize(
        &mut self,
        font_bytes: &[u8],
        font_index: u32,
        glyph_id: u16,
        px_size: f32,
        hint: bool,
    ) -> Option<GlyphRaster> {
        let font = swash::FontRef::from_index(font_bytes, font_index as usize)?;
        let mut scaler = self.inner
            .builder(font)
            .size(px_size)
            .hint(hint)
            .build();

        // Per-source iteration is the policy, not a workaround for it:
        // a `Render::new(&SOURCE_PRIORITY)` slice form short-circuits
        // at the first source whose `has_X()` table-presence gate
        // passes, but the gate doesn't check whether any glyph data
        // actually lives in that table. Proggy has empty outline
        // tables (gate passes) and a populated EBDT bitmap strike
        // (which the slice form never reaches). Iterating per source
        // and treating "succeeded with `(0, 0)` placement" as "this
        // source has no data for this glyph" routes correctly on
        // such fonts. Empty placement on every source is also the
        // correct return for legitimately-empty glyphs (space,
        // zero-width joiner, format-only chars) — the consumer
        // advances the pen via glyph metrics regardless.
        for source in &SOURCE_PRIORITY {
            let image = swash::scale::Render::new(std::slice::from_ref(source))
                .format(zeno::Format::Alpha)
                .render(&mut scaler, glyph_id);
            if let Some(image) = image {
                if image.placement.width > 0 && image.placement.height > 0 {
                    // swash's `placement.left` / `placement.top`
                    // follow the FreeType convention: `left` =
                    // pen-relative x of the bitmap's left edge;
                    // `top` = baseline-relative y of the bitmap's
                    // top edge (positive = up). These map straight
                    // into our [`GlyphRaster::bearing_x`] /
                    // `bearing_y`.
                    return Some(GlyphRaster {
                        width: image.placement.width,
                        height: image.placement.height,
                        bearing_x: image.placement.left,
                        bearing_y: image.placement.top,
                        pixels: image.data,
                    });
                }
            }
        }
        None
    }

    /// Look up the glyph id for `c` in the font's character map.
    /// Returns `None` if the font fails to parse; returns the font's
    /// `.notdef` glyph (typically id 0) when `c` is not mapped.
    pub fn glyph_id_for_char(
        &self,
        font_bytes: &[u8],
        font_index: u32,
        c: char,
    ) -> Option<u16> {
        let font = swash::FontRef::from_index(font_bytes, font_index as usize)?;
        Some(font.charmap().map(c))
    }
}

impl Default for RasterContext {
    fn default() -> Self {
        Self::new()
    }
}
