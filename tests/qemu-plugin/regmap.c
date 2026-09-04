#include "regmap.h"

#include <stdio.h>
#include <string.h>

/* Fixed order, matching TRACE_FORMAT.md §5.3. Ids are 256 + index. */
static const char *const sysreg_names[CDT_NUM_SYSREG] = {
    "SCTLR_EL1",  "TTBR0_EL1", "TTBR1_EL1",   "TCR_EL1",
    "MAIR_EL1",   "VBAR_EL1",  "ESR_EL1",     "FAR_EL1",
    "ELR_EL1",    "SPSR_EL1",  "SP_EL0",      "SP_EL1",
    "TPIDR_EL0",  "TPIDR_EL1", "TPIDRRO_EL0", "CONTEXTIDR_EL1",
    "CPACR_EL1",  "AMAIR_EL1", "PAR_EL1",     "CNTKCTL_EL1",
};

const char *regmap_sysreg_name(unsigned index)
{
    return index < CDT_NUM_SYSREG ? sysreg_names[index] : "?";
}

uint64_t regmap_normalise_pstate(uint64_t raw_cpsr)
{
    /*
     * QEMU exposes aarch64 PSTATE in `cpsr` already in architectural layout
     * (NZCV at 31..28, DAIF at 9..6, M[3:0] at 3..0). Keep only the fields the
     * emulator models; everything else is QEMU-internal or AArch32 residue.
     */
    uint64_t nzcv = (raw_cpsr >> 28) & 0xf;
    uint64_t daif = (raw_cpsr >> 6) & 0xf;
    uint64_t mode = raw_cpsr & 0xf;
    uint64_t el = (mode >> 2) & 0x3;
    uint64_t spsel = mode & CDT_PSTATE_SPSEL_MASK;

    return (nzcv << CDT_PSTATE_NZCV_SHIFT) | (daif << CDT_PSTATE_DAIF_SHIFT) |
           (el << CDT_PSTATE_EL_SHIFT) | spsel;
}

static int add_slot(struct regmap *map, qemu_plugin_reg_descriptor *desc,
                    uint16_t reg_id, uint8_t width, bool is_vector)
{
    if (map->n_slots >= REGMAP_MAX_SLOTS) {
        return -1;
    }
    unsigned index = map->n_slots++;
    map->slots[index] = (struct regmap_slot){
        .handle = desc->handle,
        .reg_id = reg_id,
        .byte_width = width,
        .is_vector = is_vector,
    };
    return (int)index;
}

static bool match_gpr(struct regmap *map, qemu_plugin_reg_descriptor *desc)
{
    for (unsigned n = 0; n < CDT_NUM_GPR; n++) {
        char want[8];
        snprintf(want, sizeof(want), "x%u", n);
        if (strcmp(desc->name, want) == 0) {
            map->gpr[n] = add_slot(map, desc, (uint16_t)(CDT_REG_X0 + n), 8,
                                   false);
            return true;
        }
    }
    return false;
}

static bool match_vreg(struct regmap *map, qemu_plugin_reg_descriptor *desc)
{
    for (unsigned n = 0; n < CDT_NUM_VREG; n++) {
        char want[8];
        snprintf(want, sizeof(want), "v%u", n);
        if (strcmp(desc->name, want) == 0) {
            uint16_t id = (uint16_t)(CDT_REG_V_BASE + 2 * n);
            map->vreg[n] = add_slot(map, desc, id, 16, true);
            map->has_vregs = true;
            return true;
        }
    }
    return false;
}

static bool match_sysreg(struct regmap *map, qemu_plugin_reg_descriptor *desc)
{
    for (unsigned n = 0; n < CDT_NUM_SYSREG; n++) {
        if (strcmp(desc->name, sysreg_names[n]) == 0) {
            uint16_t id = (uint16_t)(CDT_REG_SYS_BASE + n);
            map->sysreg[n] = add_slot(map, desc, id, 8, false);
            map->has_sysregs = true;
            return true;
        }
    }
    return false;
}

static bool match_named(struct regmap *map, qemu_plugin_reg_descriptor *desc)
{
    struct {
        const char *name;
        uint16_t id;
        uint8_t width;
        int *dest;
    } table[] = {
        { "sp",    CDT_REG_SP,     8, &map->sp },
        { "pc",    CDT_REG_PC,     8, &map->pc },
        { "cpsr",  CDT_REG_PSTATE, 4, &map->pstate },
        { "fpcr",  CDT_REG_FPCR,   4, &map->fpcr },
        { "fpsr",  CDT_REG_FPSR,   4, &map->fpsr },
    };
    for (unsigned i = 0; i < sizeof(table) / sizeof(table[0]); i++) {
        if (strcmp(desc->name, table[i].name) == 0 && *table[i].dest < 0) {
            *table[i].dest = add_slot(map, desc, table[i].id, table[i].width,
                                      false);
            return true;
        }
    }
    return false;
}

static void reset_indices(struct regmap *map)
{
    memset(map, 0, sizeof(*map));
    for (unsigned i = 0; i < CDT_NUM_GPR; i++) map->gpr[i] = -1;
    for (unsigned i = 0; i < CDT_NUM_VREG; i++) map->vreg[i] = -1;
    for (unsigned i = 0; i < CDT_NUM_SYSREG; i++) map->sysreg[i] = -1;
    map->sp = map->pc = map->pstate = map->fpcr = map->fpsr = -1;
}

/* Lowest scope at which a register is scanned on the per-block hot path. */
static enum regmap_scope slot_scope(const struct regmap_slot *slot)
{
    if (slot->reg_id >= CDT_REG_SYS_BASE) return REGMAP_SCOPE_ALL;
    if (slot->reg_id >= CDT_REG_V_BASE) return REGMAP_SCOPE_FP;
    if (slot->reg_id == CDT_REG_FPCR || slot->reg_id == CDT_REG_FPSR) {
        return REGMAP_SCOPE_FP;
    }
    return REGMAP_SCOPE_CORE;
}

/* Stable partition of slots[] so block-scanned slots occupy a prefix. The
 * index tables are rebuilt afterwards because entries move. */
static void partition_by_scope(struct regmap *map, enum regmap_scope scope)
{
    struct regmap_slot ordered[REGMAP_MAX_SLOTS];
    unsigned count = 0;
    for (unsigned i = 0; i < map->n_slots; i++) {
        if (slot_scope(&map->slots[i]) <= scope) ordered[count++] = map->slots[i];
    }
    map->n_block_slots = count;
    for (unsigned i = 0; i < map->n_slots; i++) {
        if (slot_scope(&map->slots[i]) > scope) ordered[count++] = map->slots[i];
    }
    memcpy(map->slots, ordered, count * sizeof(ordered[0]));
}

static void reindex(struct regmap *map)
{
    for (unsigned i = 0; i < CDT_NUM_GPR; i++) map->gpr[i] = -1;
    for (unsigned i = 0; i < CDT_NUM_VREG; i++) map->vreg[i] = -1;
    for (unsigned i = 0; i < CDT_NUM_SYSREG; i++) map->sysreg[i] = -1;
    map->sp = map->pc = map->pstate = map->fpcr = map->fpsr = -1;

    for (unsigned i = 0; i < map->n_slots; i++) {
        uint16_t id = map->slots[i].reg_id;
        int index = (int)i;
        if (id < CDT_NUM_GPR) map->gpr[id] = index;
        else if (id == CDT_REG_SP) map->sp = index;
        else if (id == CDT_REG_PC) map->pc = index;
        else if (id == CDT_REG_PSTATE) map->pstate = index;
        else if (id == CDT_REG_FPCR) map->fpcr = index;
        else if (id == CDT_REG_FPSR) map->fpsr = index;
        else if (id >= CDT_REG_SYS_BASE &&
                 id < CDT_REG_SYS_BASE + CDT_NUM_SYSREG) {
            map->sysreg[id - CDT_REG_SYS_BASE] = index;
        } else if (id >= CDT_REG_V_BASE) {
            map->vreg[(id - CDT_REG_V_BASE) / 2] = index;
        }
    }
}

void regmap_build(struct regmap *map, enum regmap_scope scope)
{
    reset_indices(map);

    GArray *regs = qemu_plugin_get_registers();
    for (guint i = 0; i < regs->len; i++) {
        qemu_plugin_reg_descriptor *desc =
            &g_array_index(regs, qemu_plugin_reg_descriptor, i);
        if (match_gpr(map, desc)) continue;
        if (match_named(map, desc)) continue;
        if (match_vreg(map, desc)) continue;
        match_sysreg(map, desc);
    }
    g_array_free(regs, TRUE);

    partition_by_scope(map, scope);
    reindex(map);
}

bool regmap_parse_scope(const char *text, enum regmap_scope *scope)
{
    if (strcmp(text, "core") == 0) { *scope = REGMAP_SCOPE_CORE; return true; }
    if (strcmp(text, "fp") == 0)   { *scope = REGMAP_SCOPE_FP;   return true; }
    if (strcmp(text, "all") == 0)  { *scope = REGMAP_SCOPE_ALL;  return true; }
    return false;
}

const char *regmap_scope_name(enum regmap_scope scope)
{
    switch (scope) {
        case REGMAP_SCOPE_CORE: return "core";
        case REGMAP_SCOPE_FP:   return "fp";
        case REGMAP_SCOPE_ALL:  return "all";
    }
    return "?";
}

bool regmap_read(const struct regmap_slot *slot, GByteArray *buf,
                 uint64_t *lo, uint64_t *hi)
{
    g_byte_array_set_size(buf, 0);
    if (!qemu_plugin_read_register(slot->handle, buf)) {
        return false;
    }
    uint64_t low = 0, high = 0;
    for (guint i = 0; i < buf->len && i < 8; i++) {
        low |= (uint64_t)buf->data[i] << (8 * i);
    }
    for (guint i = 8; i < buf->len && i < 16; i++) {
        high |= (uint64_t)buf->data[i] << (8 * (i - 8));
    }
    *lo = low;
    *hi = high;
    return true;
}
