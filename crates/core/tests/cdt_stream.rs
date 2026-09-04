//! The emitted CDT stream is checked by the differ that will consume it.
//!
//! Asserting byte offsets in Rust would only prove this crate agrees with
//! itself. `tests/differ/reader.mjs` is the consumer at the M1 gate, so these
//! tests write a real stream and hand it to that reader; a layout mistake fails
//! here rather than at the gate.

#![cfg(feature = "trace")]

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use coracle_core::pstate::{ExceptionLevel, Nzcv};
use coracle_core::reg::{trace_reg_id, Gpr, Vec as VecReg};
use coracle_core::regfile::RegFile;
use coracle_core::trace::writer::{deltas_between, CdtWriter, FpMode};
use coracle_core::trace::{DisconType, EndReason, ExceptionEvent, MarkerKind, RegDelta, TraceSink};

/// Runs a short program under a machine that only moves registers, and returns
/// the stream it emitted.
fn emit_sample_stream() -> Vec<u8> {
    let mut writer = CdtWriter::new("coracle-m1-test", FpMode::Precise);
    let mut regs = RegFile::new();
    let mut previous = None::<RegFile>;
    let mut deltas = Vec::new();

    writer.on_marker(MarkerKind::TraceStart, 0, 0);

    for step in 0..3u64 {
        let pc = 0x4000_0000 + step * 4;
        regs.set_pc(pc + 4);
        regs.write_x(Gpr::X(step as u8), 0x1000 + step);
        deltas_between(previous.as_ref(), &regs, &mut deltas);
        writer.on_block(pc, step, &deltas);
        previous = Some(regs.clone());
    }

    regs.pstate.nzcv = Nzcv::from_bits(0b1010);
    regs.pstate.el = ExceptionLevel::El1;
    // Distinct non-zero values: the dense exception record has no per-field
    // ids, so only distinguishable contents pin the field order.
    regs.fpcr = 0x00c0_0000;
    regs.fpsr = 0x0800_0010;
    regs.write_v(VecReg::new(5), 0x0011_2233_4455_6677_8899_aabb_ccdd_eeff);
    writer.on_exception(&ExceptionEvent {
        icount: 3,
        from_pc: 0x4000_000c,
        to_pc: 0x200,
        discon: DisconType::Exception,
        regs: &regs,
    });

    writer.finish(EndReason::Normal);
    writer.take()
}

/// Parses `bytes` with the project's own differ reader and returns its JSON
/// summary of the records.
fn read_with_differ(bytes: &[u8]) -> serde_json::Value {
    // Tests run in parallel, so the path is unique per call; a shared directory
    // lets one test's cleanup delete another's trace mid-read.
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "coracle-cdt-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let trace_path = dir.join("emitted.cdt");
    std::fs::File::create(&trace_path)
        .and_then(|mut file| file.write_all(bytes))
        .expect("write trace");

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root");
    let script = repo_root.join("tests/differ/dump_records.mjs");

    let output = Command::new("node")
        .arg(&script)
        .arg(&trace_path)
        .output()
        .expect("run the differ reader; node 22 is a CI prerequisite");

    assert!(
        output.status.success(),
        "differ reader rejected the stream: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    serde_json::from_slice(&output.stdout).expect("reader emitted JSON")
}

#[test]
fn the_differ_reads_the_header_this_emulator_writes() {
    let summary = read_with_differ(&emit_sample_stream());
    let header = &summary["header"];

    assert_eq!(header["formatVersion"], 1);
    assert_eq!(header["producer"], 2, "producer id 2 is the emulator");
    assert_eq!(header["producerName"], "coracle-m1-test");
    // tests/EMULATOR_INTERFACE.md §3: a mismatch here makes the differ refuse
    // to run rather than report spurious divergences.
    assert_eq!(header["cpuFeatureId"], "0x665fe771b9605c07");
}

#[test]
fn m1_streams_declare_precise_fp_and_no_system_registers() {
    let summary = read_with_differ(&emit_sample_stream());
    let flags = &summary["flags"];

    assert_eq!(flags["preciseFp"], true);
    assert_eq!(flags["blockDeltas"], true);
    assert_eq!(flags["hasVregs"], true);
    // TRACE_FORMAT.md §5.3: M0/M1 streams set HAS_SYSREGS = 0.
    assert_eq!(flags["hasSysregs"], false);
}

#[test]
fn every_block_record_covers_exactly_one_instruction() {
    let summary = read_with_differ(&emit_sample_stream());
    let blocks = summary["blocks"].as_array().expect("block records");

    assert_eq!(blocks.len(), 3);
    for (step, block) in blocks.iter().enumerate() {
        assert_eq!(block["nInsns"], 1, "M1 emits one instruction per block");
        assert_eq!(block["icount"], step as u64);
        assert_eq!(
            block["pc"].as_str().expect("pc"),
            format!("0x{:016x}", 0x4000_0000u64 + step as u64 * 4)
        );
    }
}

#[test]
fn the_first_block_carries_a_full_register_set_and_later_ones_only_changes() {
    let summary = read_with_differ(&emit_sample_stream());
    let blocks = summary["blocks"].as_array().expect("block records");

    // No previous state is known, so every register is emitted: 31 GPRs, SP,
    // PC, PSTATE, FPCR, FPSR and 64 V halves.
    let first = blocks[0]["deltas"].as_array().expect("deltas");
    assert_eq!(first.len(), 31 + 5 + 64);

    // The second instruction writes one register and advances PC, and nothing
    // else moved.
    let second = blocks[1]["deltas"].as_array().expect("deltas");
    let ids: Vec<u64> = second
        .iter()
        .map(|delta| delta["regId"].as_u64().expect("regId"))
        .collect();
    assert_eq!(
        ids,
        vec![trace_reg_id::gpr(1) as u64, trace_reg_id::PC as u64]
    );
}

#[test]
fn the_exception_record_carries_normalised_pstate_and_full_vector_state() {
    let summary = read_with_differ(&emit_sample_stream());
    let exception = &summary["exceptions"][0];

    assert_eq!(exception["disconType"], 2);
    assert_eq!(exception["icount"], 3);
    assert_eq!(exception["fromPc"], "0x000000004000000c");
    assert_eq!(exception["toPc"], "0x0000000000000200");

    // NZCV = 1010 at bits 31..28, CurrentEL = 1 at bits 3..2, nothing else.
    assert_eq!(exception["pstate"], "0x00000000a0000004");

    assert_eq!(exception["fpcr"], "0x0000000000c00000");
    assert_eq!(exception["fpsr"], "0x0000000008000010");
    // TRACE_FORMAT.md §5.3: M1 clears HAS_SYSREGS and writes these as zero.
    assert!(exception["sysreg"]
        .as_array()
        .expect("sysreg array")
        .iter()
        .all(|word| word == "0x0000000000000000"));

    assert_eq!(exception["v"][10], "0x8899aabbccddeeff", "v5 low half");
    assert_eq!(exception["v"][11], "0x0011223344556677", "v5 high half");
}

#[test]
fn the_end_record_reports_every_instruction_retired() {
    let summary = read_with_differ(&emit_sample_stream());

    assert_eq!(summary["end"]["icount"], 3);
    assert_eq!(summary["end"]["reason"], 0);
}

#[test]
fn an_unchanged_register_file_produces_no_deltas() {
    let regs = RegFile::new();
    let mut deltas: Vec<RegDelta> = Vec::new();

    deltas_between(Some(&regs), &regs, &mut deltas);

    assert!(
        deltas.is_empty(),
        "EMULATOR_INTERFACE.md §2: emit a register only when it changed"
    );
}
