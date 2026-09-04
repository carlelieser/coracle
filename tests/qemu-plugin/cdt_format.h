/*
 * Coracle differential trace (CDT) v1 wire format.
 * Normative description: tests/TRACE_FORMAT.md
 */
#ifndef CORACLE_CDT_FORMAT_H
#define CORACLE_CDT_FORMAT_H

#include <stdint.h>

#define CDT_MAGIC          "CORACLE\x01"
#define CDT_FORMAT_VERSION 1

enum cdt_producer {
    CDT_PRODUCER_QEMU = 1,
    CDT_PRODUCER_CORACLE = 2,
};

enum cdt_stream_flags {
    CDT_FLAG_PRECISE_FP   = 1u << 0,
    CDT_FLAG_HAS_VREGS    = 1u << 1,
    CDT_FLAG_HAS_SYSREGS  = 1u << 2,
    CDT_FLAG_BLOCK_DELTAS = 1u << 3,
};

enum cdt_record_type {
    CDT_REC_BLOCK     = 1,
    CDT_REC_EXCEPTION = 2,
    CDT_REC_MARKER    = 3,
    CDT_REC_END       = 4,
};

enum cdt_reg_id {
    CDT_REG_X0     = 0,
    CDT_REG_SP     = 31,
    CDT_REG_PC     = 32,
    CDT_REG_PSTATE = 33,
    CDT_REG_FPCR   = 34,
    CDT_REG_FPSR   = 35,
    CDT_REG_V_BASE = 64,
    CDT_REG_SYS_BASE = 256,
};

#define CDT_NUM_GPR     31   /* x0..x30 */
#define CDT_NUM_VREG    32
#define CDT_NUM_SYSREG  20

enum cdt_marker_kind {
    CDT_MARKER_TRACE_START = 1,
    CDT_MARKER_RESET       = 2,
    CDT_MARKER_ANNOTATION  = 3,
};

enum cdt_end_reason {
    CDT_END_NORMAL = 0,
    CDT_END_LIMIT  = 1,
    CDT_END_HALT   = 2,
};

#pragma pack(push, 1)

struct cdt_file_header {
    uint8_t  magic[8];
    uint32_t format_version;
    uint32_t producer;
    uint64_t flags;
    uint64_t cpu_feature_id;
    uint8_t  producer_name[32];
    uint64_t reserved[2];
};

struct cdt_record_header {
    uint8_t  type;
    uint8_t  flags;
    uint16_t length;
    uint32_t reserved;
};

struct cdt_reg_delta {
    uint16_t reg_id;
    uint16_t pad;
    uint32_t pad2;
    uint64_t value;
};

struct cdt_block_record {
    struct cdt_record_header hdr;
    uint64_t pc;
    uint64_t icount;
    uint16_t n_insns;
    uint16_t pad[3];
    /* struct cdt_reg_delta deltas[hdr.flags]; */
};

struct cdt_exception_record {
    struct cdt_record_header hdr;
    uint64_t icount;
    uint64_t from_pc;
    uint64_t to_pc;
    uint32_t discon_type;
    uint32_t pad;
    uint64_t x[32];          /* x0..x30, then SP */
    uint64_t pc;
    uint64_t pstate;
    uint64_t sysreg[CDT_NUM_SYSREG];
    uint64_t fpcr;
    uint64_t fpsr;
    uint64_t v[CDT_NUM_VREG * 2];
};

struct cdt_marker_record {
    struct cdt_record_header hdr;
    uint64_t icount;
    uint64_t kind;
    uint64_t value;
};

struct cdt_end_record {
    struct cdt_record_header hdr;
    uint64_t icount;
    uint64_t reason;
};

#pragma pack(pop)

/* PSTATE normalisation (TRACE_FORMAT.md §6). */
#define CDT_PSTATE_NZCV_SHIFT 28
#define CDT_PSTATE_DAIF_SHIFT 6
#define CDT_PSTATE_EL_SHIFT   2
#define CDT_PSTATE_SPSEL_MASK 1u

/* FNV-1a, 64-bit. Offset basis and prime are the standard values; the
 * emulator must reproduce them exactly (EMULATOR_INTERFACE.md §3). */
#define CDT_FNV_OFFSET_BASIS 0xcbf29ce484222325ULL
#define CDT_FNV_PRIME        0x100000001b3ULL

static inline uint64_t cdt_fnv1a(const char *text)
{
    uint64_t hash = CDT_FNV_OFFSET_BASIS;
    while (*text) {
        hash ^= (uint8_t)*text++;
        hash *= CDT_FNV_PRIME;
    }
    return hash;
}

#endif /* CORACLE_CDT_FORMAT_H */
