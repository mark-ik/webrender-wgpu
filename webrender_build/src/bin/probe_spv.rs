/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Inspects a SPIR-V file and shows every instruction that references a
//! given ID. Used to investigate naga `InvalidId(N)` errors — find the
//! reference and see if there's a corresponding definition.
//!
//! Usage:
//!   cargo run -p webrender_build --features shader-reflect --bin probe_spv \
//!       <path/to/file.spv> <id>

use std::fs;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: probe_spv <path> <id>");
    let target_id: u32 = args
        .next()
        .expect("usage: probe_spv <path> <id>")
        .parse()
        .expect("id must be u32");

    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    println!("=== SPIR-V header ===");
    println!("Magic:   {:#010x} (expected 0x07230203)", words[0]);
    println!("Version: {}.{}", (words[1] >> 16) & 0xff, (words[1] >> 8) & 0xff);
    println!("Gen:     {:#x}", words[2]);
    println!("Bound:   {} (max ID = {})", words[3], words[3] - 1);
    println!("Words:   {}", words.len());

    let mut idx = 5; // after header
    let mut defs: Vec<(usize, u32, Vec<u32>)> = Vec::new();
    let mut uses: Vec<(usize, u32, Vec<u32>)> = Vec::new();

    while idx < words.len() {
        let header = words[idx];
        let opcode = header & 0xffff;
        let wc = (header >> 16) as usize;
        if wc == 0 {
            println!("(bad word count at idx {})", idx);
            break;
        }
        let end = idx + wc;
        if end > words.len() {
            println!("(truncated instruction at idx {})", idx);
            break;
        }
        let operands: Vec<u32> = words[idx + 1..end].to_vec();

        // Check if any operand references target.
        let any_ref = operands.contains(&target_id);
        if any_ref {
            // Heuristic: the result id (when present) is at a known position
            // depending on opcode. For our triage, we just record the
            // reference; differentiating defs vs uses is below.
            uses.push((idx, opcode, operands.clone()));
        }

        // For type/var-producing opcodes, the result ID is at operand[0]
        // (e.g. OpTypeVoid/OpTypeFloat/OpType*) or operand[1] (e.g.
        // OpConstant/OpVariable/OpFunction which have Result Type first).
        // We identify defs by checking both common positions and the
        // opcode kind.
        let is_def = match opcode {
            // Opcodes with Result ID at operand[0] (no Result Type prefix).
            17 | 18 | 19 | 20 | 21 | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29
            | 30 | 31 | 32 | 33 | 34 | 35 | 39 | 40 => operands.first() == Some(&target_id),
            // Opcodes with Result Type at operand[0], Result ID at operand[1].
            _ => operands.get(1) == Some(&target_id),
        };
        if is_def {
            defs.push((idx, opcode, operands.clone()));
        }

        idx = end;
    }

    println!("\n=== Defs of ID {} ({}) ===", target_id, defs.len());
    for (i, op, ops) in &defs {
        println!("  word_idx {:>5}  Op{:<3} (wc={}) operands={:?}",
            i, op, ops.len() + 1, ops);
        if let Some(name) = opcode_name(*op) {
            println!("                  ({})", name);
        }
    }

    println!("\n=== Uses of ID {} ({}) ===", target_id, uses.len());
    for (i, op, ops) in &uses {
        println!("  word_idx {:>5}  Op{:<3} (wc={}) operands={:?}",
            i, op, ops.len() + 1, ops);
        if let Some(name) = opcode_name(*op) {
            println!("                  ({})", name);
        }
    }

    if defs.is_empty() {
        println!(
            "\n*** ID {} is referenced but never defined — naga's InvalidId is correct ***",
            target_id
        );
        println!("*** This is a glslang/shaderc bug: it emitted SPIR-V referencing an undefined ID ***");
    } else {
        println!(
            "\n*** ID {} IS defined (above) but naga claims InvalidId — likely a naga bug ***",
            target_id
        );
        println!("*** The definition opcode/operands above are the candidate naga repro ***");
    }
}

fn opcode_name(op: u32) -> Option<&'static str> {
    Some(match op {
        0 => "OpNop",
        17 => "OpTypeVoid",
        18 => "OpTypeBool",
        19 => "OpTypeInt",
        20 => "OpTypeFloat",
        21 => "OpTypeVector",
        22 => "OpTypeMatrix",
        23 => "OpTypeImage",
        24 => "OpTypeSampler",
        25 => "OpTypeSampledImage",
        26 => "OpTypeArray",
        27 => "OpTypeRuntimeArray",
        28 => "OpTypeStruct",
        29 => "OpTypeOpaque",
        30 => "OpTypePointer",
        31 => "OpTypeFunction",
        32 => "OpTypeEvent",
        33 => "OpTypeDeviceEvent",
        43 => "OpConstantTrue",
        44 => "OpConstantFalse",
        45 => "OpConstant",
        46 => "OpConstantComposite",
        47 => "OpConstantSampler",
        48 => "OpConstantNull",
        54 => "OpFunction",
        55 => "OpFunctionParameter",
        56 => "OpFunctionEnd",
        57 => "OpFunctionCall",
        59 => "OpVariable",
        61 => "OpLoad",
        62 => "OpStore",
        65 => "OpAccessChain",
        66 => "OpInBoundsAccessChain",
        70 => "OpName",
        71 => "OpMemberName",
        72 => "OpString",
        73 => "OpLine",
        78 => "OpExtInstImport",
        79 => "OpExtInst",
        86 => "OpDecorate",
        87 => "OpMemberDecorate",
        93 => "OpVectorShuffle",
        100 => "OpCompositeConstruct",
        101 => "OpCompositeExtract",
        103 => "OpSampledImage",
        105 => "OpImageSampleImplicitLod",
        106 => "OpImageSampleExplicitLod",
        109 => "OpImageFetch",
        110 => "OpImage",
        111 => "OpImageQueryFormat",
        // Many more — only ones likely relevant.
        248 => "OpLabel",
        253 => "OpReturn",
        254 => "OpReturnValue",
        318 => "OpExecutionMode",
        _ => return None,
    })
}
