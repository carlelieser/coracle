//! The decoded-instruction cache.
//!
//! `docs/plan.md` names this a day-one requirement and the mitigation for the
//! "interpreter too slow to boot" risk. Decoding an A64 word is a chain of
//! bit-field tests through a group decoder; a warm loop should pay that once
//! per instruction, not once per iteration.
//!
//! Direct-mapped and tagged by the full 32-bit encoding rather than by PC.
//! Tagging by encoding is what makes the cache correct without invalidation:
//! if the bytes at a PC change, the tag no longer matches and the entry is
//! re-decoded. M2 adds the executed-page bitmap for the JIT, but the
//! interpreter needs no help from it.

use crate::decode::{decode, Instruction};

/// Entries in the cache. A power of two so indexing is a mask.
///
/// 4096 entries covers a 16 KiB instruction footprint, which is more than the
/// hot loop of any M1 benchmark and comfortably more than a libc `memcpy`.
pub const DECODE_CACHE_ENTRIES: usize = 4096;

/// Direct-mapped cache of decoded instructions.
#[derive(Debug, Clone)]
pub struct DecodeCache {
    tags: [u32; DECODE_CACHE_ENTRIES],
    entries: [Instruction; DECODE_CACHE_ENTRIES],
    hits: u64,
    misses: u64,
}

/// The encoding that means "this slot holds nothing".
///
/// `UDF #0`, architecturally permanently undefined, so a real guest word can
/// never collide with it; if one somehow did, the miss path would decode it to
/// the same [`crate::decode::Op::Unallocated`] anyway.
const EMPTY_TAG: u32 = 0x0000_0000;

impl Default for DecodeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DecodeCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self {
            tags: [EMPTY_TAG; DECODE_CACHE_ENTRIES],
            entries: [crate::decode::unallocated(EMPTY_TAG); DECODE_CACHE_ENTRIES],
            hits: 0,
            misses: 0,
        }
    }

    /// The decoded form of `encoding`, decoding it on a miss.
    ///
    /// `pc` selects the slot; the encoding is the tag. Two PCs holding the same
    /// word decode identically, so a collision between them costs nothing but a
    /// re-decode.
    pub fn decoded(&mut self, pc: u64, encoding: u32) -> Instruction {
        let slot = Self::slot(pc);
        if self.tags[slot] == encoding && encoding != EMPTY_TAG {
            self.hits += 1;
            return self.entries[slot];
        }

        self.misses += 1;
        let insn = decode(encoding);
        self.tags[slot] = encoding;
        self.entries[slot] = insn;
        insn
    }

    /// Lookups served without decoding.
    pub const fn hits(&self) -> u64 {
        self.hits
    }

    /// Lookups that had to decode.
    pub const fn misses(&self) -> u64 {
        self.misses
    }

    const fn slot(pc: u64) -> usize {
        // Instructions are 4-byte aligned, so the low two bits carry nothing.
        (pc >> 2) as usize & (DECODE_CACHE_ENTRIES - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Op;

    #[test]
    fn a_repeated_lookup_at_one_pc_decodes_only_once() {
        let mut cache = DecodeCache::new();
        // add x0, x0, #1
        let encoding = 0x9100_0400;

        for _ in 0..100 {
            assert_eq!(cache.decoded(0x1000, encoding).op, Op::Add);
        }

        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 99);
    }

    #[test]
    fn rewriting_the_word_at_a_pc_re_decodes_it_without_explicit_invalidation() {
        let mut cache = DecodeCache::new();

        assert_eq!(cache.decoded(0x1000, 0x9100_0400).op, Op::Add);
        // sub x0, x0, #1 at the same address.
        assert_eq!(cache.decoded(0x1000, 0xd100_0400).op, Op::Sub);

        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn two_pcs_that_alias_one_slot_evict_each_other_but_stay_correct() {
        let mut cache = DecodeCache::new();
        let stride = DECODE_CACHE_ENTRIES as u64 * 4;

        for _ in 0..10 {
            assert_eq!(cache.decoded(0x1000, 0x9100_0400).op, Op::Add);
            assert_eq!(cache.decoded(0x1000 + stride, 0xd100_0400).op, Op::Sub);
        }

        assert_eq!(cache.hits(), 0, "the two entries alias");
        assert_eq!(cache.misses(), 20);
    }

    #[test]
    fn an_all_zero_word_is_never_served_from_an_empty_slot() {
        // Slot tags start at zero, so a guest word of zero must not read as a
        // hit on a slot nothing was ever stored in.
        let mut cache = DecodeCache::new();

        let insn = cache.decoded(0x2000, 0);

        assert!(insn.op.is_unallocated());
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn a_hot_loop_that_fits_the_cache_misses_only_on_its_first_pass() {
        let mut cache = DecodeCache::new();
        let body: [u32; 4] = [0x9100_0400, 0xd100_0400, 0x8b01_0020, 0x1400_0000];

        for _ in 0..50 {
            for (index, encoding) in body.iter().enumerate() {
                cache.decoded(0x3000 + index as u64 * 4, *encoding);
            }
        }

        assert_eq!(cache.misses(), body.len() as u64);
    }
}
