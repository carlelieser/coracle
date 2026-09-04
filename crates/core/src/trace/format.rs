//! CDT v1 wire constants.
//!
//! Normative description: `tests/TRACE_FORMAT.md`. The differ at
//! `tests/differ/` and the QEMU plugin at `tests/qemu-plugin/` hardcode the
//! same numbers; this module must not diverge from them.

/// File magic, including the format-generation byte.
pub const MAGIC: [u8; 8] = *b"CORACLE\x01";

/// Format version this crate emits.
pub const FORMAT_VERSION: u32 = 1;

/// Producer id for streams this emulator writes.
pub const PRODUCER_CORACLE: u32 = 2;

/// Bytes in the file header.
pub const FILE_HEADER_BYTES: usize = 80;

/// Bytes in a record's common prologue.
pub const RECORD_HEADER_BYTES: usize = 8;

/// Bytes in one register delta.
pub const REG_DELTA_BYTES: usize = 16;

/// Bytes before the delta array in a block record.
pub const BLOCK_PREFIX_BYTES: usize = 32;

/// Bytes available for the producer name, NUL-padded.
pub const PRODUCER_NAME_BYTES: usize = 32;

/// Largest `flags` value a block record can carry, and so the most deltas one
/// record can hold. `flags` is a single byte.
pub const MAX_DELTAS_PER_BLOCK: usize = u8::MAX as usize;

/// Record type tags.
pub mod record_type {
    /// A basic block retired, with a register delta.
    pub const BLOCK: u8 = 1;
    /// Exception or interrupt entry, with full architectural state.
    pub const EXCEPTION: u8 = 2;
    /// Synchronisation or annotation.
    pub const MARKER: u8 = 3;
    /// Clean end of stream.
    pub const END: u8 = 4;
}

/// Stream-level flags in the file header.
pub mod stream_flags {
    /// Producer ran softfloat everywhere.
    pub const PRECISE_FP: u64 = 1 << 0;
    /// Block records may carry V-register deltas.
    pub const HAS_VREGS: u64 = 1 << 1;
    /// Block records may carry EL1 system-register deltas.
    pub const HAS_SYSREGS: u64 = 1 << 2;
    /// Block records are deltas. Always set in v1.
    pub const BLOCK_DELTAS: u64 = 1 << 3;
}

/// `kind` values for a marker record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// Start of trace. Required once, before the first block.
    TraceStart = 1,
    /// Machine reset.
    Reset = 2,
    /// Caller-supplied annotation.
    Annotation = 3,
}

/// Why a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Guest ran to completion.
    Normal = 0,
    /// An instruction or time limit was reached.
    LimitReached = 1,
    /// The guest halted itself.
    GuestHalt = 2,
}

/// What kind of control-flow discontinuity an exception record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconType {
    /// An asynchronous interrupt.
    Interrupt = 1,
    /// A synchronous exception.
    Exception = 2,
    /// A call out to the host: PSCI, `HVC`, `SMC`, or the M1 syscall shim.
    HostCall = 4,
}

/// The advertised CPU feature mask, as `tests/qemu_cpu.sh` defines it.
///
/// The differ refuses to compare two streams whose hashes of this string
/// disagree (`tests/EMULATOR_INTERFACE.md` §3), which is what stops a
/// feature-mask drift from surfacing as thousands of spurious divergences.
pub const FEATURE_MASK: &str = "armv8.0-a+el1+nolse+nosve+nopauth+nomte+nobti+qemucrypto";

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 64-bit FNV-1a, as `tests/EMULATOR_INTERFACE.md` §3 specifies it.
pub const fn fnv1a(text: &str) -> u64 {
    let bytes = text.as_bytes();
    let mut hash = FNV_OFFSET_BASIS;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        index += 1;
    }
    hash
}

/// The `cpu_feature_id` this emulator writes into every stream header.
pub const CPU_FEATURE_ID: u64 = fnv1a(FEATURE_MASK);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_published_test_vectors() {
        // Standard FNV-1a 64-bit vectors, so a transcription error in the
        // constants is caught without depending on the feature string.
        assert_eq!(fnv1a(""), FNV_OFFSET_BASIS);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a("foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_feature_string_is_the_one_the_harness_pins() {
        // tests/qemu_cpu.sh, CORACLE_FEATURE_ID. A change on either side must
        // be made on both, or the differ refuses to run.
        assert_eq!(
            FEATURE_MASK,
            "armv8.0-a+el1+nolse+nosve+nopauth+nomte+nobti+qemucrypto"
        );
        // Pinned so a change to either the string or the hash is a test
        // failure here rather than a refusal to compare at gate time.
        assert_eq!(CPU_FEATURE_ID, 0x665f_e771_b960_5c07);
    }

    #[test]
    fn record_sizes_are_the_multiples_of_eight_the_format_requires() {
        for size in [
            FILE_HEADER_BYTES,
            RECORD_HEADER_BYTES,
            REG_DELTA_BYTES,
            BLOCK_PREFIX_BYTES,
        ] {
            assert_eq!(size % 8, 0, "{size} is not 8-byte aligned");
        }
    }
}
