/*
 * Maps QEMU plugin register handles onto CDT register ids.
 */
#ifndef CORACLE_REGMAP_H
#define CORACLE_REGMAP_H

#include <qemu-plugin.h>
#include <stdbool.h>
#include <stdint.h>

#include "cdt_format.h"

/* Widest register we read is a 128-bit V register. */
#define REGMAP_MAX_SLOTS 512

struct regmap_slot {
    struct qemu_plugin_register *handle;
    uint16_t reg_id;      /* CDT id; V regs use the low-half id */
    uint8_t  byte_width;  /* 4, 8 or 16 */
    bool     is_vector;   /* emits two deltas */
};

/*
 * Which registers are scanned on the per-block hot path. Every scope emits
 * full architectural state at exception entry regardless; this only trades
 * per-block resolution against emission speed.
 */
enum regmap_scope {
    /* x0-x30, sp, pc, pstate. ~34 reads/block. */
    REGMAP_SCOPE_CORE = 0,
    /* CORE plus fpcr/fpsr and all 32 V registers. ~68 reads/block. */
    REGMAP_SCOPE_FP = 1,
    /* Everything, including EL1 system registers. ~140 reads/block. */
    REGMAP_SCOPE_ALL = 2,
};

struct regmap {
    struct regmap_slot slots[REGMAP_MAX_SLOTS];
    unsigned n_slots;
    /* slots[0 .. n_block_slots) are scanned per block; the rest only on
     * exception entry. regmap_build() partitions them by scope. */
    unsigned n_block_slots;
    /* Index into slots[] for the fixed-order exception dump, -1 if absent. */
    int gpr[CDT_NUM_GPR];
    int sp;
    int pc;
    int pstate;
    int fpcr;
    int fpsr;
    int vreg[CDT_NUM_VREG];
    int sysreg[CDT_NUM_SYSREG];
    bool has_vregs;
    bool has_sysregs;
};

void regmap_build(struct regmap *map, enum regmap_scope scope);

bool regmap_parse_scope(const char *text, enum regmap_scope *scope);
const char *regmap_scope_name(enum regmap_scope scope);

/* Reads one slot; hi receives the upper 64 bits of a vector register. */
bool regmap_read(const struct regmap_slot *slot, GByteArray *buf,
                 uint64_t *lo, uint64_t *hi);

const char *regmap_sysreg_name(unsigned index);

uint64_t regmap_normalise_pstate(uint64_t raw_cpsr);

#endif /* CORACLE_REGMAP_H */
