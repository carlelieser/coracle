//! Serialises trace events into a CDT v1 byte stream.
//!
//! Writes into a caller-supplied `alloc::vec::Vec<u8>`: this crate is `no_std`
//! and has no file handle, so the host drains the buffer. Every record is
//! length-prefixed and 8-byte aligned, so a partially drained buffer is always
//! cut at a record boundary.

use alloc::vec::Vec;

use super::format::{
    record_type, stream_flags, EndReason, MarkerKind, BLOCK_PREFIX_BYTES, CPU_FEATURE_ID,
    FILE_HEADER_BYTES, FORMAT_VERSION, MAGIC, MAX_DELTAS_PER_BLOCK, PRODUCER_CORACLE,
    PRODUCER_NAME_BYTES, RECORD_HEADER_BYTES, REG_DELTA_BYTES,
};
use super::sink::{ExceptionEvent, RegDelta, TraceSink};
use crate::reg::{trace_reg_id, Vec as VecReg, NUM_GPR};
use crate::regfile::RegFile;

/// System registers an exception record carries. M1 emits these as zero;
/// `HAS_SYSREGS` is clear, and M2 fills them in.
const NUM_SYSREG: usize = 20;

/// Instructions per block in M1's per-instruction mode.
///
/// `tests/EMULATOR_INTERFACE.md` §4 names this the M1 debugging mode: it costs
/// roughly 3x the trace size but puts every divergence at a single
/// instruction, and it sidesteps having to reproduce QEMU's TCG block
/// boundaries — including the page-boundary rule — before the CPU works at all.
pub const M1_INSNS_PER_BLOCK: u16 = 1;

/// Whether this build ran softfloat everywhere.
///
/// Differential legs must set it (`tests/EMULATOR_INTERFACE.md` §5); the
/// native-wasm FP backend clears it and the differ then applies the
/// NaN-payload-insensitive policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpMode {
    /// Softfloat everywhere.
    Precise,
    /// Native wasm FP while FPCR is in default mode.
    Native,
}

/// Writes CDT v1 records into a byte buffer.
#[derive(Debug)]
pub struct CdtWriter {
    buffer: Vec<u8>,
    retired: u64,
}

impl CdtWriter {
    /// Starts a stream, writing the file header.
    ///
    /// `producer_name` is truncated to 32 bytes and NUL-padded. M1 streams
    /// clear `HAS_SYSREGS`: no EL1 register exists yet, and the differ then
    /// compares only the architectural core, which is the correct scope for a
    /// user-mode gate.
    pub fn new(producer_name: &str, fp_mode: FpMode) -> Self {
        let mut writer = Self {
            buffer: Vec::new(),
            retired: 0,
        };
        writer.write_file_header(producer_name, fp_mode);
        writer
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buffer
    }

    /// Removes and returns the bytes written so far, leaving the stream open.
    ///
    /// Always cuts at a record boundary, because each record is appended whole.
    pub fn take(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.buffer)
    }

    fn write_file_header(&mut self, producer_name: &str, fp_mode: FpMode) {
        let mut flags = stream_flags::BLOCK_DELTAS | stream_flags::HAS_VREGS;
        if fp_mode == FpMode::Precise {
            flags |= stream_flags::PRECISE_FP;
        }

        self.buffer.extend_from_slice(&MAGIC);
        self.buffer.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        self.buffer
            .extend_from_slice(&PRODUCER_CORACLE.to_le_bytes());
        self.buffer.extend_from_slice(&flags.to_le_bytes());
        self.buffer.extend_from_slice(&CPU_FEATURE_ID.to_le_bytes());

        let mut name = [0u8; PRODUCER_NAME_BYTES];
        let source = producer_name.as_bytes();
        let len = source.len().min(PRODUCER_NAME_BYTES);
        name[..len].copy_from_slice(&source[..len]);
        self.buffer.extend_from_slice(&name);
        self.buffer.extend_from_slice(&[0u8; 16]);

        debug_assert_eq!(self.buffer.len(), FILE_HEADER_BYTES);
    }

    fn write_record_header(&mut self, record: u8, flags: u8, length: usize) {
        self.buffer.push(record);
        self.buffer.push(flags);
        self.buffer
            .extend_from_slice(&(length as u16).to_le_bytes());
        self.buffer.extend_from_slice(&0u32.to_le_bytes());
    }

    fn write_words(&mut self, words: impl IntoIterator<Item = u64>) {
        for word in words {
            self.buffer.extend_from_slice(&word.to_le_bytes());
        }
    }
}

impl TraceSink for CdtWriter {
    fn on_marker(&mut self, kind: MarkerKind, icount: u64, value: u64) {
        self.write_record_header(record_type::MARKER, 0, RECORD_HEADER_BYTES + 24);
        self.write_words([icount, kind as u64, value]);
    }

    fn on_block(&mut self, pc: u64, icount: u64, deltas: &[RegDelta]) {
        debug_assert!(
            deltas.len() <= MAX_DELTAS_PER_BLOCK,
            "a block record's delta count must fit the header's flags byte"
        );
        let deltas = &deltas[..deltas.len().min(MAX_DELTAS_PER_BLOCK)];

        self.retired = icount + M1_INSNS_PER_BLOCK as u64;

        let length = BLOCK_PREFIX_BYTES + deltas.len() * REG_DELTA_BYTES;
        self.write_record_header(record_type::BLOCK, deltas.len() as u8, length);
        self.write_words([pc, icount]);
        self.buffer
            .extend_from_slice(&M1_INSNS_PER_BLOCK.to_le_bytes());
        self.buffer.extend_from_slice(&[0u8; 6]);

        for delta in deltas {
            self.buffer.extend_from_slice(&delta.reg_id.to_le_bytes());
            self.buffer.extend_from_slice(&[0u8; 6]);
            self.buffer.extend_from_slice(&delta.value.to_le_bytes());
        }
    }

    fn on_exception(&mut self, event: &ExceptionEvent<'_>) {
        let length = RECORD_HEADER_BYTES + 32 + (32 + 2 + NUM_SYSREG + 2 + 64) * 8;
        self.write_record_header(record_type::EXCEPTION, 0, length);
        self.write_words([event.icount, event.from_pc, event.to_pc]);
        self.buffer
            .extend_from_slice(&(event.discon as u32).to_le_bytes());
        self.buffer.extend_from_slice(&0u32.to_le_bytes());
        write_full_state(&mut self.buffer, event.regs);
    }

    fn finish(&mut self, reason: EndReason) {
        self.write_record_header(record_type::END, 0, RECORD_HEADER_BYTES + 16);
        self.write_words([self.retired, reason as u64]);
    }
}

/// Appends the dense architectural-state block an exception record carries.
///
/// Field order is fixed by `tests/TRACE_FORMAT.md` §4.2: `x[32]` with `SP` in
/// the last slot, `pc`, normalised `pstate`, the system registers, `fpcr`,
/// `fpsr`, then 32 V registers as `(lo, hi)` pairs.
fn write_full_state(buffer: &mut Vec<u8>, regs: &RegFile) {
    for index in 0..NUM_GPR as u8 {
        buffer.extend_from_slice(&regs.x(index).to_le_bytes());
    }
    buffer.extend_from_slice(&regs.sp().to_le_bytes());
    buffer.extend_from_slice(&regs.pc().to_le_bytes());
    buffer.extend_from_slice(&regs.pstate.to_trace_word().to_le_bytes());

    // M1 has no EL1 registers; HAS_SYSREGS is clear and the differ skips them.
    buffer.extend_from_slice(&[0u8; NUM_SYSREG * 8]);

    buffer.extend_from_slice(&regs.fpcr.to_le_bytes());
    buffer.extend_from_slice(&regs.fpsr.to_le_bytes());

    for index in 0..VecReg::COUNT as u8 {
        let value = regs.read_v(VecReg::new(index));
        buffer.extend_from_slice(&(value as u64).to_le_bytes());
        buffer.extend_from_slice(&((value >> 64) as u64).to_le_bytes());
    }
}

/// Collects the deltas between two register-file snapshots.
///
/// `tests/EMULATOR_INTERFACE.md` §2 requires a register be emitted only when it
/// changed. Passing `None` for `previous` emits the full set, which is what the
/// block after an exception must do so the two streams cannot silently drift.
pub fn deltas_between(previous: Option<&RegFile>, current: &RegFile, out: &mut Vec<RegDelta>) {
    out.clear();
    let mut push = |reg_id, value, old: Option<u64>| {
        if old != Some(value) {
            out.push(RegDelta { reg_id, value });
        }
    };

    for index in 0..NUM_GPR as u8 {
        push(
            trace_reg_id::gpr(index),
            current.x(index),
            previous.map(|regs| regs.x(index)),
        );
    }
    push(trace_reg_id::SP, current.sp(), previous.map(RegFile::sp));
    push(trace_reg_id::PC, current.pc(), previous.map(RegFile::pc));
    push(
        trace_reg_id::PSTATE,
        current.pstate.to_trace_word(),
        previous.map(|regs| regs.pstate.to_trace_word()),
    );
    push(trace_reg_id::FPCR, current.fpcr, previous.map(|r| r.fpcr));
    push(trace_reg_id::FPSR, current.fpsr, previous.map(|r| r.fpsr));

    push_vec_deltas(previous, current, out);
}

fn push_vec_deltas(previous: Option<&RegFile>, current: &RegFile, out: &mut Vec<RegDelta>) {
    for index in 0..VecReg::COUNT as u8 {
        let reg = VecReg::new(index);
        let value = current.read_v(reg);
        if previous.map(|regs| regs.read_v(reg)) == Some(value) {
            continue;
        }
        out.push(RegDelta {
            reg_id: trace_reg_id::vec_lo(index),
            value: value as u64,
        });
        out.push(RegDelta {
            reg_id: trace_reg_id::vec_hi(index),
            value: (value >> 64) as u64,
        });
    }
}
