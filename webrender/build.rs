/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

extern crate webrender_build;

use std::borrow::Cow;
use std::env;
use std::fs::{canonicalize, read_dir, File};
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use webrender_build::shader::*;
use webrender_build::shader_features::{ShaderFeatureFlags, get_shader_features};

// glsopt is known to leak, but we don't particularly care.
#[no_mangle]
pub extern "C" fn __lsan_default_options() -> *const u8 {
    b"detect_leaks=0\0".as_ptr()
}

/// Compute the shader path for insertion into the include_str!() macro.
/// This makes for more compact generated code than inserting the literal
/// shader source into the generated file.
///
/// If someone is building on a network share, I'm sorry.
fn escape_include_path(path: &Path) -> String {
    let full_path = canonicalize(path).unwrap();
    let full_name = full_path.as_os_str().to_str().unwrap();
    let full_name = full_name.replace("\\\\?\\", "");
    let full_name = full_name.replace("\\", "/");

    full_name
}

fn write_unoptimized_shaders(
    mut glsl_files: Vec<PathBuf>,
    shader_file: &mut File,
) -> Result<(), std::io::Error> {
    writeln!(
        shader_file,
        "  pub static ref UNOPTIMIZED_SHADERS: HashMap<&'static str, SourceWithDigest> = {{"
    )?;
    writeln!(shader_file, "    let mut shaders = HashMap::new();")?;

    // Sort the file list so that the shaders.rs file is filled
    // deterministically.
    glsl_files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    for glsl in glsl_files {
        // Compute the shader name.
        assert!(glsl.is_file());
        let shader_name = glsl.file_name().unwrap().to_str().unwrap();
        let shader_name = shader_name.replace(".glsl", "");

        // Compute a digest of the #include-expanded shader source. We store
        // this as a literal alongside the source string so that we don't need
        // to hash large strings at runtime.
        let mut hasher = DefaultHasher::new();
        let base = glsl.parent().unwrap();
        assert!(base.is_dir());
        ShaderSourceParser::new().parse(
            Cow::Owned(shader_source_from_file(&glsl)),
            &|f| Cow::Owned(shader_source_from_file(&base.join(&format!("{}.glsl", f)))),
            &mut |s| hasher.write(s.as_bytes()),
        );
        let digest: ProgramSourceDigest = hasher.into();

        writeln!(
            shader_file,
            "    shaders.insert(\"{}\", SourceWithDigest {{ source: include_str!(\"{}\"), digest: \"{}\"}});",
            shader_name,
            escape_include_path(&glsl),
            digest,
        )?;
    }
    writeln!(shader_file, "    shaders")?;
    writeln!(shader_file, "  }};")?;

    Ok(())
}

#[derive(Clone, Debug)]
struct ShaderOptimizationInput {
    shader_name: &'static str,
    config: String,
    gl_version: ShaderVersion,
}

#[derive(Debug)]
struct ShaderOptimizationOutput {
    full_shader_name: String,
    gl_version: ShaderVersion,
    vert_file_path: PathBuf,
    frag_file_path: PathBuf,
    digest: ProgramSourceDigest,
}

#[derive(Debug)]
struct ShaderOptimizationError {
    shader: ShaderOptimizationInput,
    message: String,
}

/// Prepends the line number to each line of a shader source.
fn enumerate_shader_source_lines(shader_src: &str) -> String {
    // For some reason the glsl-opt errors are offset by 1 compared
    // to the provided shader source string.
    let mut out = format!("0\t|");
    for (n, line) in shader_src.split('\n').enumerate() {
        let line_number = n + 1;
        out.push_str(&format!("{}\t|{}\n", line_number, line));
    }
    out
}

fn write_optimized_shaders(
    shader_dir: &Path,
    shader_file: &mut File,
    out_dir: &str,
) -> Result<(), std::io::Error> {
    writeln!(
        shader_file,
        "  pub static ref OPTIMIZED_SHADERS: HashMap<(ShaderVersion, &'static str), OptimizedSourceWithDigest> = {{"
    )?;
    writeln!(shader_file, "    let mut shaders = HashMap::new();")?;

    // The full set of optimized shaders can be quite large, so only optimize
    // for the GL version we expect to be used on the target platform. If a different GL
    // version is used we will simply fall back to the unoptimized shaders.
    let shader_versions = match env::var("CARGO_CFG_TARGET_OS").as_ref().map(|s| &**s) {
        Ok("android") | Ok("windows") => [ShaderVersion::Gles],
        _ => [ShaderVersion::Gl],
    };

    let mut shaders = Vec::default();
    for &gl_version in &shader_versions {
        let mut flags = ShaderFeatureFlags::all();
        if gl_version != ShaderVersion::Gl {
            flags.remove(ShaderFeatureFlags::GL);
        }
        if gl_version != ShaderVersion::Gles {
            flags.remove(ShaderFeatureFlags::GLES);
            flags.remove(ShaderFeatureFlags::TEXTURE_EXTERNAL);
        }
        if !matches!(
            env::var("CARGO_CFG_TARGET_OS").as_ref().map(|s| &**s),
            Ok("android")
        ) {
            flags.remove(ShaderFeatureFlags::TEXTURE_EXTERNAL_ESSL1);
        }
        // The optimizer cannot handle the required EXT_YUV_target extension
        flags.remove(ShaderFeatureFlags::TEXTURE_EXTERNAL_BT709);
        flags.remove(ShaderFeatureFlags::DITHERING);

        for (shader_name, configs) in get_shader_features(flags) {
            for config in configs {
                shaders.push(ShaderOptimizationInput {
                    shader_name,
                    config,
                    gl_version,
                });
            }
        }
    }

    let outputs = build_parallel::compile_objects::<_, _, ShaderOptimizationError, _>(
        &|shader: &ShaderOptimizationInput| {
            println!("Optimizing shader {:?}", shader);
            let target = match shader.gl_version {
                ShaderVersion::Gl => glslopt::Target::OpenGl,
                ShaderVersion::Gles => glslopt::Target::OpenGles30,
            };
            let glslopt_ctx = glslopt::Context::new(target);

            let features = shader
                .config
                .split(",")
                .filter(|f| !f.is_empty())
                .collect::<Vec<_>>();

            let (vert_src, frag_src) =
                build_shader_strings(shader.gl_version, &features, shader.shader_name, &|f| {
                    Cow::Owned(shader_source_from_file(
                        &shader_dir.join(&format!("{}.glsl", f)),
                    ))
                });

            let full_shader_name = if shader.config.is_empty() {
                shader.shader_name.to_string()
            } else {
                format!("{}_{}", shader.shader_name, shader.config.replace(",", "_"))
            };

            // Compute a digest of the optimized shader sources. We store this
            // as a literal alongside the source string so that we don't need
            // to hash large strings at runtime.
            let mut hasher = DefaultHasher::new();

            let [vert_file_path, frag_file_path] = [
                (glslopt::ShaderType::Vertex, vert_src, "vert"),
                (glslopt::ShaderType::Fragment, frag_src, "frag"),
            ]
            .map(|(shader_type, shader_src, extension)| {
                let output = glslopt_ctx.optimize(shader_type, shader_src.clone());
                if !output.get_status() {
                    let source = enumerate_shader_source_lines(&shader_src);
                    return Err(ShaderOptimizationError {
                        shader: shader.clone(),
                        message: format!("{}\n{}", source, output.get_log()),
                    });
                }

                let shader_path = Path::new(out_dir).join(format!(
                    "{}_{:?}.{}",
                    full_shader_name, shader.gl_version, extension
                ));
                write_optimized_shader_file(
                    &shader_path,
                    output.get_output().unwrap(),
                    &shader.shader_name,
                    &features,
                    &mut hasher,
                );
                Ok(shader_path)
            });

            let vert_file_path = vert_file_path?;
            let frag_file_path = frag_file_path?;

            println!("Finished optimizing shader {:?}", shader);

            Ok(ShaderOptimizationOutput {
                full_shader_name,
                gl_version: shader.gl_version,
                vert_file_path,
                frag_file_path,
                digest: hasher.into(),
            })
        },
        &shaders,
    );

    match outputs {
        Ok(mut outputs) => {
            // Sort the shader list so that the shaders.rs file is filled
            // deterministically.
            outputs.sort_by(|a, b| {
                (a.gl_version, a.full_shader_name.clone())
                    .cmp(&(b.gl_version, b.full_shader_name.clone()))
            });

            for shader in outputs {
                writeln!(
                    shader_file,
                    "    shaders.insert(({}, \"{}\"), OptimizedSourceWithDigest {{",
                    shader.gl_version.variant_name(),
                    shader.full_shader_name,
                )?;
                writeln!(
                    shader_file,
                    "        vert_source: include_str!(\"{}\"),",
                    escape_include_path(&shader.vert_file_path),
                )?;
                writeln!(
                    shader_file,
                    "        frag_source: include_str!(\"{}\"),",
                    escape_include_path(&shader.frag_file_path),
                )?;
                writeln!(shader_file, "        digest: \"{}\",", shader.digest)?;
                writeln!(shader_file, "    }});")?;
            }
        }
        Err(err) => match err {
            build_parallel::Error::BuildError(err) => {
                panic!("Error optimizing shader {:?}: {}", err.shader, err.message)
            }
            _ => panic!("Error optimizing shaders."),
        },
    }

    writeln!(shader_file, "    shaders")?;
    writeln!(shader_file, "  }};")?;

    Ok(())
}

fn write_optimized_shader_file(
    path: &Path,
    source: &str,
    shader_name: &str,
    features: &[&str],
    hasher: &mut DefaultHasher,
) {
    let mut file = File::create(&path).unwrap();
    for (line_number, line) in source.lines().enumerate() {
        // We embed the shader name and features as a comment in the
        // source to make debugging easier.
        // The #version directive must be on the first line so we insert
        // the extra information on the next line.
        if line_number == 1 {
            let prelude = format!("// {}\n// features: {:?}\n\n", shader_name, features);
            file.write_all(prelude.as_bytes()).unwrap();
            hasher.write(prelude.as_bytes());
        }
        file.write_all(line.as_bytes()).unwrap();
        file.write_all("\n".as_bytes()).unwrap();
        hasher.write(line.as_bytes());
        hasher.write("\n".as_bytes());
    }
}

/// Preprocess assembled WR GLSL source for naga's Vulkan-style GLSL 4.50 frontend.
///
/// naga's GLSL frontend targets Vulkan GLSL, which differs from desktop GL GLSL in
/// several ways that require source-level patching before naga sees the source.
///
/// Transformations applied:
///
/// 1. `#version` → `#version 450`  (naga requires 450)
/// 2. `#extension` directives stripped  (naga rejects unknown extensions)
/// 3. `precision ...;` statements stripped  (GLES-only, invalid in 4.50 core)
/// 4. Combined sampler uniforms split into texture + global sampler:
///    `uniform sampler2D sName;` → `layout(binding=N, set=0) uniform texture2D sName;`
///    A single shared `layout(binding=0, set=1) uniform sampler global_sampler;` is
///    injected once, covering all texture samples.
/// 5. Remaining uniforms get `layout(binding=N, set=0)`.
/// 6. `texture(sName, coord)` call sites get the required combined-type wrapper:
///    → `texture(sampler2D(sName, global_sampler), coord)`
/// 7. The TEX_SAMPLE macro body is rewritten to use the split types so any
///    indirect `texture(sampler, ...)` in macros works correctly.
/// 8. Vertex stage: `in`/`attribute` declarations without `layout(location=N)` get
///    sequential location qualifiers, fixing the BindingCollision validation error.
/// 9. Fragment stage: `out vec4` declarations without `layout(location=N)` get
///    `layout(location=0)`.
///
/// Precision qualifiers (`highp`, `mediump`, `lowp`) are stripped from every
/// emitted declaration: they are valid in GLES but illegal in GLSL 450 core.
#[cfg(feature = "wgpu_backend")]
fn strip_precision(s: &str) -> String {
    // Precision qualifiers always precede a type token, so they are always
    // followed by a space.  A simple token-delete is safe here.
    let mut out = s.to_string();
    for q in &["highp ", "mediump ", "lowp "] {
        while out.contains(q) {
            out = out.replace(q, "");
        }
    }
    out
}

/// Tokens that begin control-flow statements or storage-qualifier declarations
/// rather than user-defined function definitions.
#[cfg(feature = "wgpu_backend")]
const NOT_FUNC_START: &[&str] = &[
    "if", "else", "for", "while", "do", "switch", "return",
    "struct", "uniform", "in", "out", "varying", "attribute",
    "flat", "smooth", "noperspective", "layout", "PER_INSTANCE",
];

/// Strip the trailing `// comment` portion of a GLSL source line.
#[cfg(feature = "wgpu_backend")]
#[inline]
fn strip_glsl_comment(s: &str) -> &str {
    match s.find("//") { Some(i) => s[..i].trim_end(), None => s }
}

/// For each function PROTOTYPE found in the assembled GLSL source
/// (`rettype name(params);` at brace depth 0, single- or multi-line),
/// find the corresponding DEFINITION (`rettype name(params) { … }`) that
/// appears AFTER the prototype, and move it to the position of the prototype.
///
/// This gives naga's GLSL frontend the definition-before-use ordering it
/// requires even though standard GLSL forward declarations (prototypes) are
/// present.  WebRender uses this pattern in `brush.glsl` / `ps_quad.glsl` where
/// a prototype is declared, `main()` calls it, and the specific shader (e.g.
/// `brush_solid.glsl`) appended later provides the body.
///
/// Unmatched prototypes are left in place so naga can report a useful error.
/// Definitions that have no prototype are left in their original position.
///
/// ## Algorithm
///
/// For each chunk of consecutive lines at brace-depth 0 whose first token is
/// not a keyword (control flow, storage qualifier, etc.):
///   - Track paren depth to identify the end of the parameter list.
///   - If the parameter list ends with `;` (no body) → **prototype** chunk.
///   - If the parameter list is followed by a `{ … }` body → **definition** chunk.
///
/// The paren-balanced accumulation correctly handles signatures spanning many
/// lines (e.g. `void brush_vs(\n    param1,\n    param2\n);`).
#[cfg(feature = "wgpu_backend")]
fn move_definitions_before_prototypes(src: &str) -> String {
    use std::collections::{HashMap, HashSet};

    let lines: Vec<&str> = src.lines().collect();
    let n = lines.len();

    // ── Pass 1: collect prototype chunks and definition chunks ────────────────
    // Each item: (function_name, start_line_incl, end_line_excl)
    let mut prototypes: Vec<(String, usize, usize)> = Vec::new();
    let mut defs:       Vec<(String, usize, usize)> = Vec::new();

    // Brace depth for blocks that are NOT function definitions (struct, uniform
    // block, etc.).  When this is > 0 we skip everything.
    let mut block_depth: i32 = 0;
    let mut i = 0;

    while i < n {
        let code = strip_glsl_comment(lines[i].trim());

        if block_depth > 0 {
            for c in code.chars() {
                match c { '{' => block_depth += 1, '}' => block_depth -= 1, _ => {} }
            }
            i += 1;
            continue;
        }

        // Skip blank lines, preprocessor directives, and comment lines.
        if code.is_empty() || code.starts_with('#') || code.starts_with("//") {
            i += 1;
            continue;
        }

        let first = code.split_whitespace().next().unwrap_or("");

        if NOT_FUNC_START.contains(&first) {
            // Non-function declaration (struct, uniform block, etc.) — track braces.
            for c in code.chars() {
                match c { '{' => block_depth += 1, '}' => block_depth -= 1, _ => {} }
            }
            i += 1;
            continue;
        }

        // Reject lines without `(` — variable declarations without initializer,
        // `return` without parens, closing `}` of a block, etc.
        if !code.contains('(') {
            for c in code.chars() {
                match c { '{' => block_depth += 1, '}' => block_depth -= 1, _ => {} }
            }
            i += 1;
            continue;
        }

        // Reject assignment statements: `=` appearing before the first `(`.
        // These are variable declarations with initializer (`Type v = fn(…);`),
        // not function signatures.
        {
            let paren = code.find('(').unwrap();
            if code[..paren].contains('=') {
                for c in code.chars() {
                    match c { '{' => block_depth += 1, '}' => block_depth -= 1, _ => {} }
                }
                i += 1;
                continue;
            }
        }

        // ── Paren-balanced accumulation ──────────────────────────────────────
        // Accumulate lines starting at `i` until we classify the chunk as a
        // prototype (ends with `;` after balanced parens) or definition (has a
        // `{ … }` body after balanced parens).
        let chunk_start = i;
        let mut paren_depth: i32 = 0;
        let mut brace_depth: i32 = 0;
        let mut sig_closed  = false;  // paren_depth returned to 0
        let mut body_open   = false;  // `{` seen after sig_closed
        let mut j = i;

        'accum: while j < n && (j - chunk_start) < 200 {
            let lc = strip_glsl_comment(lines[j].trim());
            for c in lc.chars() {
                match c {
                    '(' => paren_depth += 1,
                    ')' => {
                        paren_depth -= 1;
                        if paren_depth <= 0 { paren_depth = 0; sig_closed = true; }
                    }
                    '{' if sig_closed => {
                        brace_depth += 1;
                        body_open = true;
                    }
                    '}' if body_open => {
                        brace_depth -= 1;
                        if brace_depth <= 0 {
                            j += 1;
                            break 'accum;  // definition body complete
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
            // Prototype: signature balanced and this line ends the statement.
            if sig_closed && !body_open && (lc.ends_with(';') || lc.ends_with(");")) {
                break 'accum;
            }
        }

        if !sig_closed {
            // Could not parse — treat as unknown, advance by 1.
            for c in code.chars() {
                match c { '{' => block_depth += 1, '}' => block_depth -= 1, _ => {} }
            }
            i += 1;
            continue;
        }

        // Extract function name from the first line of the chunk.
        let sig_first = strip_glsl_comment(lines[chunk_start].trim());
        let name_opt = sig_first.find('(')
            .map(|p| sig_first[..p].trim_end())
            .and_then(|before| before.split_whitespace().last())
            .map(|s| s.to_string());

        if let Some(name) = name_opt {
            if body_open && brace_depth <= 0 {
                defs.push((name, chunk_start, j));
            } else if !body_open {
                prototypes.push((name, chunk_start, j));
            }
        }

        i = j;
    }

    // ── Pass 2: build move table ──────────────────────────────────────────────
    // For each prototype, find the LAST definition for that name that appears
    // AFTER the prototype.  Only consider definitions that come after.
    let mut def_by_name: HashMap<String, (usize, usize)> = HashMap::new();
    for (name, start, end) in &defs {
        // Last definition with that name wins (HashMap insert overwrites).
        def_by_name.insert(name.clone(), (*start, *end));
    }

    // Collect names where we'll actually do a move.
    let names_to_move: Vec<String> = prototypes
        .iter()
        .filter_map(|(name, ps, _pe)| {
            def_by_name.get(name.as_str())
                .filter(|(ds, _)| ds > ps)
                .map(|_| name.clone())
        })
        .collect();

    if names_to_move.is_empty() {
        return src.to_string();
    }

    // Build skip sets and replacement map.
    let mut skip_lines: HashSet<usize> = HashSet::new();
    // Map: prototype_start_line → (def_start, def_end, proto_end)
    let mut replacements: HashMap<usize, (usize, usize, usize)> = HashMap::new();

    for name in &names_to_move {
        let (ps, pe) = prototypes.iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, s, e)| (*s, *e))
            .unwrap();
        let (ds, de) = def_by_name[name.as_str()];

        // Skip all prototype lines (they get replaced) and definition
        // lines (they get moved to the prototype position).
        for li in ps..pe { skip_lines.insert(li); }
        for li in ds..de { skip_lines.insert(li); }

        replacements.insert(ps, (ds, de, pe));
    }

    // ── Pass 3: reconstruct ──────────────────────────────────────────────────
    let mut result = String::with_capacity(src.len() + 512);
    let mut li = 0;
    while li < n {
        if let Some(&(ds, de, pe)) = replacements.get(&li) {
            // Emit the definition body in place of the prototype.
            for l in ds..de {
                result.push_str(lines[l]);
                result.push('\n');
            }
            // Jump past the prototype range.
            li = pe;
            continue;
        }
        if !skip_lines.contains(&li) {
            result.push_str(lines[li]);
            result.push('\n');
        }
        li += 1;
    }
    result
}

/// Scan the leading tokens of a GLSL declaration line and return the first
/// GLSL storage/interface qualifier found.
///
/// The function skips known non-storage qualifiers and WR-specific macros that
/// can appear before the storage qualifier (e.g. `flat`, `PER_INSTANCE`).
/// Returns `None` if the first non-prefix token is not a storage qualifier —
/// meaning this is not an interface variable declaration.
#[cfg(feature = "wgpu_backend")]
fn storage_qual(code: &str) -> Option<&'static str> {
    for token in code.split_whitespace() {
        match token {
            "in"        => return Some("in"),
            "out"       => return Some("out"),
            "varying"   => return Some("varying"),
            "attribute" => return Some("attribute"),
            // Allowed to precede the storage qualifier:
            // interpolation qualifiers (GLSL built-in) and WR instance macros.
            "flat" | "smooth" | "noperspective" | "PER_INSTANCE" => {}
            _ => return None,
        }
    }
    None
}

#[cfg(feature = "wgpu_backend")]
fn preprocess_for_naga(src: &str, stage: naga::ShaderStage) -> String {
    use std::collections::{HashMap, HashSet};

    // ── Combined-sampler type table ──────────────────────────────────────────
    // Maps the GLSL combined sampler type keyword to the separate Vulkan-GLSL
    // texture type.  The constructor keyword (used in `texture()` wrappers) is
    // the same as the combined type.
    const SAMPLER_TYPES: &[(&str, &str)] = &[
        ("sampler2D",      "texture2D"),
        ("isampler2D",     "itexture2D"),
        ("usampler2D",     "utexture2D"),
        ("sampler2DArray", "texture2DArray"),
        ("sampler2DRect",  "texture2DRect"),
        ("sampler2DMS",    "texture2DMS"),
        ("samplerCube",    "textureCube"),
    ];

    // ── Pre-scan: identify combined-sampler variable names ───────────────────
    // Build a set of uniform variable names whose type is a combined sampler so
    // that Pass 2 can rewrite `texture(sName, ...)` call sites.
    let mut sampler_names: HashSet<String>  = HashSet::new();
    // Map: sampler var-name → combined type ("sampler2D", "isampler2D", ...)
    let mut sampler_type_map: HashMap<String, &'static str> = HashMap::new();

    for raw_line in src.lines() {
        let trimmed = raw_line.trim_start();
        let code = match trimmed.find("//") {
            Some(i) => trimmed[..i].trim_end(),
            None => trimmed,
        };
        if !code.starts_with("uniform ") || !code.ends_with(';') {
            continue;
        }
        let after_uniform = &code["uniform ".len()..code.len() - 1];
        let tokens: Vec<&str> = after_uniform.split_whitespace().collect();
        for &(samp_ty, _tex_ty) in SAMPLER_TYPES {
            if tokens.contains(&samp_ty) {
                if let Some(&name) = tokens.last() {
                    sampler_names.insert(name.to_string());
                    // First-occurrence wins: the true #ifdef branch always
                    // precedes the false #elif branch in the assembled source.
                    sampler_type_map.entry(name.to_string()).or_insert(samp_ty);
                }
            }
        }
    }

    // ── Pass 1: line-by-line rewriting ───────────────────────────────────────
    let mut name_to_binding: HashMap<String, u32> = HashMap::new();
    let mut next_binding: u32 = 0;
    let mut next_attr_loc: u32 = 0;     // vertex attribute input locations
    let mut next_vary_loc: u32 = 0;     // varying interface locations (vertex out / fragment in)
    let is_vertex   = stage == naga::ShaderStage::Vertex;
    let is_fragment = stage == naga::ShaderStage::Fragment;
    let mut global_sampler_injected = false;

    let mut out: Vec<String> = Vec::with_capacity(src.lines().count() + 4);

    for raw_line in src.lines() {
        let trimmed = raw_line.trim_start();
        let code = match trimmed.find("//") {
            Some(i) => trimmed[..i].trim_end(),
            None => trimmed,
        };
        let indent = &raw_line[..raw_line.len() - trimmed.len()];

        if trimmed.starts_with("#version") {
            out.push("#version 450".to_string());
            // Inject the shared sampler right after the version so it is
            // available to all shader stages.
            if !global_sampler_injected {
                out.push("layout(binding = 0, set = 1) uniform sampler global_sampler;".to_string());
                global_sampler_injected = true;
            }

        } else if trimmed.starts_with("#extension") {
            // naga rejects unknown #extension directives even in dead branches.

        } else if code.starts_with("precision ") && code.ends_with(';') {
            // GLES precision statements are invalid in GLSL 4.50 core.

        } else if code.starts_with("uniform ") && code.ends_with(';')
            && !code.starts_with("uniform struct ")
        {
            // Determine the variable name (last whitespace-delimited token).
            let var_name = code.trim_end_matches(';')
                .split_whitespace()
                .last()
                .unwrap_or("unknown")
                .to_string();

            // Assign a stable binding index.
            let binding = *name_to_binding.entry(var_name.clone()).or_insert_with(|| {
                let b = next_binding;
                next_binding += 1;
                b
            });

            if let Some(&samp_ty) = sampler_type_map.get(&var_name) {
                // Replace the combined sampler type with the Vulkan-GLSL texture type.
                // Drop precision qualifiers (HIGHP_SAMPLER_FLOAT macro etc.) — they
                // are irrelevant for texture2D/itexture2D declarations.
                let tex_ty = SAMPLER_TYPES
                    .iter()
                    .find(|&&(s, _)| s == samp_ty)
                    .map(|&(_, t)| t)
                    .unwrap_or("texture2D");
                out.push(format!(
                    "{}layout(binding = {}, set = 0) uniform {} {};",
                    indent, binding, tex_ty, var_name
                ));
            } else {
                out.push(format!(
                    "{}layout(binding = {}, set = 0) {}",
                    indent, binding, strip_precision(trimmed)
                ));
            }

        } else if code.ends_with(';') && !code.contains("layout(") {
            // Detect interface variable declarations:
            // [flat | smooth | noperspective | PER_INSTANCE] [varying | in | out | attribute] ...;
            // These need explicit location qualifiers to prevent BindingCollision.
            match storage_qual(code) {
                Some("attribute") | Some("in") if is_vertex => {
                    // Vertex attribute inputs — unique sequential locations.
                    out.push(format!("{}layout(location = {}) {}", indent, next_attr_loc, strip_precision(trimmed)));
                    next_attr_loc += 1;
                }
                Some("in") | Some("varying") if is_fragment => {
                    // Fragment varying inputs — must match vertex varying output locations.
                    out.push(format!("{}layout(location = {}) {}", indent, next_vary_loc, strip_precision(trimmed)));
                    next_vary_loc += 1;
                }
                Some("out") | Some("varying") if is_vertex => {
                    // Vertex varying outputs — must match fragment varying input locations.
                    out.push(format!("{}layout(location = {}) {}", indent, next_vary_loc, strip_precision(trimmed)));
                    next_vary_loc += 1;
                }
                Some("out") if is_fragment => {
                    // Fragment render-target output: WR uses a single colour target.
                    out.push(format!("{}layout(location = 0) {}", indent, strip_precision(trimmed)));
                }
                _ => {
                    out.push(raw_line.to_string());
                }
            }

        } else if trimmed.starts_with("#define TEX_SAMPLE(") {
            // Rewrite the macro body so the `sampler` parameter (which will be a
            // `texture2D` variable) is wrapped with the required combined-type
            // constructor before passing it to `texture()`.
            //
            // Original:  texture(sampler, tex_coord.xy)
            // Rewritten: texture(sampler2D(sampler, global_sampler), tex_coord.xy)
            let rewritten = raw_line.replace(
                "texture(sampler, ",
                "texture(sampler2D(sampler, global_sampler), ",
            );
            out.push(rewritten);

        } else {
            out.push(raw_line.to_string());
        }
    }

    let intermediate = out.join("\n");

    // ── Pass 2: rewrite texture() call sites ─────────────────────────────────
    // For each known sampler variable, replace the direct `texture(sName, ...)` form
    // with the Vulkan-GLSL `texture(sampler2D(sName, global_sampler), ...)` wrapper.
    // texelFetch / texelFetchOffset / textureSize work with bare texture2D and are
    // left untouched.
    let mut result = intermediate;
    for samp_name in &sampler_names {
        let old = format!("texture({},", samp_name);
        // Determine the combined-type constructor: sampler2D for float samplers,
        // isampler2D for integer, etc.  In practice WR never calls texture() on
        // integer samplers (they only use texelFetch), so sampler2D is always correct
        // for the texture() wrapper.
        let new = format!("texture(sampler2D({}, global_sampler),", samp_name);
        result = result.replace(&old, &new);
    }

    // Global precision-qualifier strip: highp/mediump/lowp are GLES-only and
    // invalid in GLSL 4.50 core.  They can appear inside function bodies and
    // struct/uniform blocks where the per-line Pass 1 handler doesn't reach.
    result = strip_precision(&result);

    // NOTE: move_definitions_before_prototypes() was tested here (Stage 4c) but
    // had to be deactivated.  WR's brush / ps_quad ForwardDependency pattern
    // involves definitions that depend on struct types defined in the same
    // "specific" shader file (e.g. SolidBrush in brush_solid.glsl).  Moving
    // only the function body — without also moving the struct and helper
    // functions it depends on — produces UnknownVariable errors.
    //
    // Fixing ForwardDependency correctly requires either:
    //   a) Full topological sort of all global declarations, OR
    //   b) Restructuring build_shader_strings so "specific" shader content
    //      (struct + helper_fn + brush_vs body) is assembled BEFORE the driver
    //      file's brush_shader_main_vs / main rather than after.
    // That work is deferred to a later stage.

    result
}

/// Translate fully-assembled GLSL source to WGSL via naga 26.
/// Returns `Ok(wgsl)` on success, or `Err(diagnostic)` if naga rejects the shader.
/// Callers should emit `cargo:warning` for failures and skip the variant.
#[cfg(feature = "wgpu_backend")]
fn translate_to_wgsl(
    glsl: &str,
    stage: naga::ShaderStage,
    name: &str,
    config: &str,
) -> Result<String, String> {
    use naga::{
        back::wgsl,
        front::glsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    // naga's validator can panic on certain malformed intermediate IR (e.g.,
    // index-out-of-bounds in the flow analyser).  Catch any internal panic and
    // convert it to a graceful skip-with-warning so the build never crashes.
    let glsl_owned = glsl.to_string();
    let name_s = name.to_string();
    let config_s = config.to_string();

    let outcome = std::panic::catch_unwind(move || {
        let module = glsl::Frontend::default()
            .parse(&glsl::Options::from(stage), &glsl_owned)
            .map_err(|e| format!(
                "GLSL->naga parse failed [shader={} config={:?}]: {:?}", name_s, config_s, e
            ))?;
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .map_err(|e| format!(
                "naga validation failed [shader={} config={:?}]: {:?}", name_s, config_s, e
            ))?;
        wgsl::write_string(&module, &info, wgsl::WriterFlags::empty()).map_err(|e| format!(
            "WGSL emit failed [shader={} config={:?}]: {:?}", name_s, config_s, e
        ))
    });

    match outcome {
        Ok(inner) => inner,
        Err(panic_val) => {
            let msg = if let Some(s) = panic_val.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = panic_val.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "unknown panic".to_string()
            };
            Err(format!("naga panicked [shader={} config={:?}]: {}", name, config, msg))
        }
    }
}

/// Generate WGSL shaders for the wgpu backend and write the `WGSL_SHADERS`
/// lazy_static entry into `shader_file`.
#[cfg(feature = "wgpu_backend")]
fn write_wgsl_shaders(
    shader_dir: &Path,
    out_dir: &str,
    shader_file: &mut File,
) -> Result<(), std::io::Error> {
    use std::fs;
    use webrender_build::shader_features::wgpu_shader_feature_flags;

    let wgsl_dir = Path::new(out_dir).join("wgsl");
    fs::create_dir_all(&wgsl_dir)?;

    writeln!(
        shader_file,
        "  pub static ref WGSL_SHADERS: HashMap<(&'static str, &'static str), WgslShaderSource> = {{"
    )?;
    writeln!(shader_file, "    let mut shaders = HashMap::new();")?;

    let features = get_shader_features(wgpu_shader_feature_flags());

    // Sort for deterministic output.
    let mut sorted_features: Vec<(&str, Vec<String>)> = features
        .iter()
        .map(|(&name, configs)| (name, configs.clone()))
        .collect();
    sorted_features.sort_by_key(|(name, _)| *name);

    let mut entries: Vec<(String, String, PathBuf, PathBuf)> = Vec::new();
    let mut success_count: u32 = 0;
    let mut fail_count: u32 = 0;

    for (shader_name, configs) in &sorted_features {
        let mut sorted_configs = configs.clone();
        sorted_configs.sort();
        for config in &sorted_configs {
            let feature_list: Vec<&str> = config
                .split(',')
                .filter(|f| !f.is_empty())
                .collect();
            let (vert_glsl, frag_glsl) = build_shader_strings(
                ShaderVersion::Gl,
                &feature_list,
                shader_name,
                &|f| Cow::Owned(shader_source_from_file(&shader_dir.join(format!("{}.glsl", f)))),
            );

            // Preprocess: version-bump, strip #extension and precision,
            // split sampler2D declarations, assign locations and bindings.
            let vert_glsl = preprocess_for_naga(&vert_glsl, naga::ShaderStage::Vertex);
            let frag_glsl = preprocess_for_naga(&frag_glsl, naga::ShaderStage::Fragment);

            let vert_wgsl = translate_to_wgsl(
                &vert_glsl,
                naga::ShaderStage::Vertex,
                shader_name,
                config,
            );
            let frag_wgsl = translate_to_wgsl(
                &frag_glsl,
                naga::ShaderStage::Fragment,
                shader_name,
                config,
            );

            // Filesystem-safe key: replace commas with underscores.
            let safe_key = if config.is_empty() {
                shader_name.to_string()
            } else {
                format!("{}__{}", shader_name, config.replace(',', "_"))
            };

            match (vert_wgsl, frag_wgsl) {
                (Ok(vert), Ok(frag)) => {
                    let vert_path = wgsl_dir.join(format!("{}_vs.wgsl", safe_key));
                    let frag_path = wgsl_dir.join(format!("{}_fs.wgsl", safe_key));
                    fs::write(&vert_path, &vert)?;
                    fs::write(&frag_path, &frag)?;
                    entries.push((
                        shader_name.to_string(),
                        config.clone(),
                        vert_path,
                        frag_path,
                    ));
                    success_count += 1;
                }
                (vert_res, frag_res) => {
                    let msg = vert_res.err().or_else(|| frag_res.err()).unwrap_or_default();
                    println!(
                        "cargo:warning=WGSL translation skipped [{}#{}]: {}",
                        shader_name, config, msg
                    );
                    fail_count += 1;
                }
            }
        }
    }

    // Stage 4b adds sampler2D splitting, attribute locations, and texture() wrappers.
    // Expect a significant portion of variants to succeed; complex shaders with
    // sampler2D function parameters (cs_svg_filter*) will still skip gracefully.
    println!(
        "cargo:warning=WGSL translation: {}/{} variants succeeded",
        success_count,
        success_count + fail_count
    );

    for (name, config, vert_path, frag_path) in &entries {
        writeln!(
            shader_file,
            "    shaders.insert((\"{name}\", \"{config}\"), WgslShaderSource {{ \
                vert_source: include_str!(\"{vp}\"), \
                frag_source: include_str!(\"{fp}\") \
            }});",
            name = name,
            config = config,
            vp = escape_include_path(vert_path),
            fp = escape_include_path(frag_path),
        )?;
    }

    writeln!(shader_file, "    shaders")?;
    writeln!(shader_file, "  }};")?;

    Ok(())
}

/// Stub for GL builds: `write_wgsl_shaders` is never called in GL builds
/// but must exist so the call-site in `main()` compiles in both configs.
#[cfg(not(feature = "wgpu_backend"))]
fn write_wgsl_shaders(
    _shader_dir: &Path,
    _out_dir: &str,
    _shader_file: &mut File,
) -> Result<(), std::io::Error> {
    unreachable!()
}

fn main() -> Result<(), std::io::Error> {
    // Enforce that exactly one rendering backend is selected.
    let gl_backend = std::env::var("CARGO_FEATURE_GL_BACKEND").is_ok();
    let wgpu_backend = std::env::var("CARGO_FEATURE_WGPU_BACKEND").is_ok();
    if gl_backend && wgpu_backend {
        panic!("gl_backend and wgpu_backend are mutually exclusive; enable exactly one");
    }
    if !gl_backend && !wgpu_backend {
        panic!("exactly one of gl_backend or wgpu_backend must be enabled");
    }

    let out_dir = env::var("OUT_DIR").unwrap_or("out".to_owned());

    let shaders_file_path = Path::new(&out_dir).join("shaders.rs");
    let mut glsl_files = vec![];

    println!("cargo:rerun-if-changed=res");
    let res_dir = Path::new("res");
    for entry in read_dir(res_dir)? {
        let entry = entry?;
        let path = entry.path();

        if entry.file_name().to_str().unwrap().ends_with(".glsl") {
            println!("cargo:rerun-if-changed={}", path.display());
            glsl_files.push(path.to_owned());
        }
    }

    let mut shader_file = File::create(shaders_file_path)?;

    writeln!(shader_file, "/// AUTO GENERATED BY build.rs\n")?;
    writeln!(shader_file, "use std::collections::HashMap;\n")?;
    writeln!(shader_file, "use webrender_build::shader::ShaderVersion;\n")?;
    writeln!(shader_file, "pub struct SourceWithDigest {{")?;
    writeln!(shader_file, "    pub source: &'static str,")?;
    writeln!(shader_file, "    pub digest: &'static str,")?;
    writeln!(shader_file, "}}\n")?;
    writeln!(shader_file, "pub struct OptimizedSourceWithDigest {{")?;
    writeln!(shader_file, "    pub vert_source: &'static str,")?;
    writeln!(shader_file, "    pub frag_source: &'static str,")?;
    writeln!(shader_file, "    pub digest: &'static str,")?;
    writeln!(shader_file, "}}\n")?;
    if !gl_backend {
        writeln!(shader_file, "pub struct WgslShaderSource {{")?;
        writeln!(shader_file, "    pub vert_source: &'static str,")?;
        writeln!(shader_file, "    pub frag_source: &'static str,")?;
        writeln!(shader_file, "}}\n")?;
    }
    writeln!(shader_file, "lazy_static! {{")?;

    if gl_backend {
        write_unoptimized_shaders(glsl_files, &mut shader_file)?;
        writeln!(shader_file, "")?;
        write_optimized_shaders(&res_dir, &mut shader_file, &out_dir)?;
    } else {
        // wgpu_backend: emit empty GL maps; generate WGSL shaders via naga.
        writeln!(shader_file, "  pub static ref UNOPTIMIZED_SHADERS: HashMap<&'static str, SourceWithDigest> = HashMap::new();")?;
        writeln!(shader_file, "  pub static ref OPTIMIZED_SHADERS: HashMap<(ShaderVersion, &'static str), OptimizedSourceWithDigest> = HashMap::new();")?;
        write_wgsl_shaders(&res_dir, &out_dir, &mut shader_file)?;
    }
    writeln!(shader_file, "}}")?;

    Ok(())
}
