# Survey: 8 remaining GL-coupled types in trait surface

Date: 2026-05-01
Branch: `spirv-shader-pipeline`
Status: investigation finding — feeds the next P1 step

After three lift waves (P1b/c/d), `traits.rs` still imports 8 types from
`super::gl::`. This doc surveys each one's renderer-side footprint and
recommends the conversion path. Findings in order of conversion ease
(easiest first).

## Summary table

| Type | Outside-`device/` usage | Recommended action | Renderer-side impact |
| --- | --- | --- | --- |
| `UniformLocation` | None | Remove unused import | Zero |
| `Stream<'a>` | None | Convert to GAT `type Stream<'a>;` | Zero |
| `FBOId` | 1 field on `Renderer` | Convert to assoc type `type RenderTargetHandle;` | One field type change |
| `Program` | ~5 sites (debug.rs, shade.rs) | Already assoc type in trait; renderer call sites need migration to `<D as GpuShaders>::Program` | Moderate |
| `ReadTarget` | 6 sites (renderer/mod.rs, screen_capture.rs) | Variants reference FBOId; cascades. Either assoc type or stay-concrete-on-renderer. | Moderate |
| `DrawTarget` | 7+ sites with variant construction (composite.rs, renderer/mod.rs) | Same as ReadTarget | Moderate-large |
| `ExternalTexture` | ~6 sites incl. constructors + map fields | Assoc type would need parallel `WgpuExternalTexture::new()` constructors | Large |
| `UploadPBOPool` | ~5 sites incl. ownership in `Renderer` field | Assoc type ripples deeply through pool ownership | Large |

## Detailed per-type findings

### 1. `UniformLocation` — easiest

**Outside-`device/`:** Grep returned zero hits.
**Inside trait:** Listed in `traits.rs` `super::gl::` import but only
`Self::UniformLocation` is used in trait method signatures.
**Recommendation:** Confirmed-dead import. Delete from `traits.rs`. No
other change needed; the assoc type machinery is already in place.

### 2. `Stream<'a>` — easy

**Outside-`device/`:** Zero hits. (Two grep matches were false positives:
`VertexUsageHint::Stream` enum variant, and the word "stream" in a doc
comment.)
**Inside trait:** Used in
`fn create_custom_vao(streams: &[Stream<'_>]) -> Self::CustomVao`.
**Recommendation:** Convert to GAT `type Stream<'a>;` on `GpuResources`.
Renderer code is unaffected. The `create_custom_vao` signature becomes
`fn create_custom_vao(&mut self, streams: &[Self::Stream<'_>]) -> Self::CustomVao`.
Each backend defines what `Stream` looks like (GL: existing `Stream<'a>`
struct with `VBOId`; wgpu: a `(BufferRef, &[VertexAttribute])` pair or similar).
Verify GAT lifetime works on a `&[Self::Stream<'_>]` slice — this is the
tricky bit; may need a different shape (e.g. a slice of references).

### 3. `FBOId` — small, good pattern-establisher

**Outside-`device/`:** Single hit:

- [`renderer/mod.rs:843`](../webrender/src/renderer/mod.rs#L843):
  `read_fbo: FBOId,` — one field on `Renderer`.

**Inside trait:** Returned by `create_fbo()`, `create_fbo_for_external_texture()`;
taken by `delete_fbo`, `bind_external_draw_target`. Embedded inside
`ReadTarget` and `DrawTarget` enum variants.
**Recommendation:** Convert to assoc type `type RenderTargetHandle;` on
`GpuResources`. Renderer's single `read_fbo` field changes to either
(a) the concrete GL type via the type alias (`Device::RenderTargetHandle`
where `Device = GlDevice` via the alias from P0c), or (b) a generic field
when renderer goes generic.
This is the **first conversion to do** — pattern is the same as for the
other types but minimal blast radius lets us get it right cheaply.
**Cascade:** `ReadTarget` and `DrawTarget` reference `FBOId` in their
variants, so converting FBOId to an assoc type forces those to either
also become assoc types or accept generic parameterization (e.g.
`enum ReadTarget<H> { Texture { fbo_id: H }, ... }` with `H = Self::RenderTargetHandle`).

### 4. `Program` — already assoc type, but renderer uses concrete

**Outside-`device/`:**

- [`renderer/debug.rs`](../webrender/src/renderer/debug.rs): `font_program: Program`,
  `color_program: Program` (struct fields).
- [`renderer/shade.rs`](../webrender/src/renderer/shade.rs): `program: Option<Program>`
  (field), function signatures using `Program` and `Result<Program, ShaderError>`.

**Inside trait:** Already declared as `type Program;` on `GpuShaders`. The
import in `traits.rs` is leftover from before the assoc-type change and
is unused — same as `UniformLocation`. Can be removed.
**Renderer migration:** Renderer files name `Program` concretely in field
types. With the type alias `pub type Device = GlDevice;` from P0c, those
references work today. If renderer goes generic, fields become e.g.
`<D as GpuShaders>::Program`, which is verbose. Realistic path: leave
renderer using the concrete `Program` (via the alias) and migrate to
generic only when wgpu impl actually drives the renderer.
**Recommendation:** Drop the unused `Program` import from `traits.rs`.
Don't touch renderer code yet.

### 5. `ReadTarget` — variants leak GL handles

**Outside-`device/`:**

- [`renderer/mod.rs`](../webrender/src/renderer/mod.rs): 4 calls to
  `ReadTarget::from_texture(...)`, used as method args.
- [`screen_capture.rs`](../webrender/src/screen_capture.rs):
  `ReadTarget::Default`, `ReadTarget::from_texture(...)`, etc.

**Inside trait:** Used in `bind_read_target(target: ReadTarget)`,
`blit_render_target` and `blit_render_target_invert_y`.
**Cascade from FBOId:** Variants reference `FBOId`. If `FBOId` becomes
assoc type, `ReadTarget` either follows or gets parameterized over the
handle type.
**Recommendation:** Defer until after the FBOId conversion lands, then
re-evaluate. Two options:

- **Option A**: `type ReadTarget;` — each backend defines its own enum.
  Renderer code that calls `ReadTarget::from_texture(...)` becomes
  backend-aware (no longer portable).
- **Option B**: `pub enum ReadTarget<H> { ... fbo_id: H ... }` lifted
  to `types.rs`, parameterized by `Self::RenderTargetHandle`. Renderer
  code calls `ReadTarget::<H>::from_texture(...)` where H is fixed by
  the backend.

Option B preserves renderer portability at the cost of a type parameter
floating around. Option A is the minimal-design path. Decide once we
see how Option B feels for FBOId alone.

### 6. `DrawTarget` — same shape as ReadTarget but more sites

**Outside-`device/`:** ~7 sites with variant construction in
[`renderer/composite.rs`](../webrender/src/renderer/composite.rs)
and [`renderer/mod.rs`](../webrender/src/renderer/mod.rs):
`DrawTarget::NativeSurface { ... }`, `DrawTarget::Default { ... }`,
field types `draw_target: DrawTarget`, function args.
**Recommendation:** Same as ReadTarget; convert in lockstep with FBOId.

### 7. `ExternalTexture` — owned by renderer in maps

**Outside-`device/`:**

- [`renderer/external_image.rs`](../webrender/src/renderer/external_image.rs):
  Two `ExternalTexture::new(...)` constructor calls; map argument types.
- [`renderer/mod.rs`](../webrender/src/renderer/mod.rs):
  `external_images: FastHashMap<DeferredResolveIndex, ExternalTexture>`,
  `owned_external_images: FastHashMap<(ExternalImageId, u8), ExternalTexture>`.

**Inside trait:** Used in
`fn bind_external_texture<S>(&mut self, slot: S, external_texture: &ExternalTexture)`
on `GpuPass`. Also constructed by `delete_external_texture` (inherent-only,
not on trait).
**Recommendation:** Convert to `type ExternalTexture;` on `GpuResources`.
Renderer's `FastHashMap<..., ExternalTexture>` becomes
`FastHashMap<..., D::ExternalTexture>` if renderer goes generic, or stays
concrete via the type alias. Constructor `ExternalTexture::new(...)` is a
problem: it can't be called on an associated type. Two options:

- Add a constructor method to `GpuResources`:
  `fn create_external_texture(&mut self, ...) -> Self::ExternalTexture`
- Keep `ExternalTexture` concrete-on-Device and accept that the
  constructor is GL-only

This is large enough to defer and design separately.

### 8. `UploadPBOPool` — owned by renderer, deeply embedded

**Outside-`device/`:**

- [`renderer/init.rs`](../webrender/src/renderer/init.rs):
  `UploadPBOPool::new(&mut device, ...)` — constructor.
- [`renderer/mod.rs`](../webrender/src/renderer/mod.rs):
  `texture_upload_pbo_pool: UploadPBOPool` field on `Renderer`.
- [`renderer/vertex.rs`](../webrender/src/renderer/vertex.rs):
  `pbo_pool: &mut UploadPBOPool` method args.

**Inside trait:** Used in
`fn upload_texture<'a>(&mut self, pbo_pool: &'a mut UploadPBOPool) -> Self::TextureUploader<'a>`
on `GpuResources`.
**Recommendation:** Convert to `type UploadPbo;` on `GpuResources`.
Same constructor problem as `ExternalTexture` — `UploadPBOPool::new(...)`
needs to either become `device.create_upload_pbo_pool(...)` or stay
concrete. Defer; design carefully.

## Recommended next step

Three small wins, then one pattern-establisher, then defer the rest:

1. **Drop dead imports.** `UniformLocation` and `Program` in
   `traits.rs` are unused (only `Self::Program` / `Self::UniformLocation`
   referenced). Cleanup commit.
2. **Convert `Stream<'a>` to GAT.** Zero renderer impact; tests the GAT
   lifetime pattern. This is the smallest assoc-type conversion possible.
3. **Convert `FBOId` to `type RenderTargetHandle`.** One renderer field
   touched. Establishes the pattern with minimum risk. Triggers a
   parallel decision on `ReadTarget`/`DrawTarget` shape (assoc type vs.
   parameterized lift) — the choice is easier with a working FBOId
   conversion as reference.
4. **Defer `ReadTarget` / `DrawTarget` / `ExternalTexture` / `UploadPBOPool`
   until after FBOId.** Their conversions are larger and the design
   choice (assoc type vs. parameterized lift) is informed by what we
   learn from FBOId.

After step 3, traits.rs's `super::gl::` import drops to 4 names
(`ReadTarget`, `DrawTarget`, `ExternalTexture`, `UploadPBOPool`),
and we have a clear pattern + working example for the remaining
conversions.
