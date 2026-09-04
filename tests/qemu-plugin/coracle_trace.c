/*
 * QEMU TCG plugin emitting a Coracle differential trace (CDT v1).
 *
 * Usage:
 *   qemu-system-aarch64 ... -plugin ./libcoracle_trace.so,out=trace.cdt[,limit=N]
 *
 * Format: tests/TRACE_FORMAT.md
 */
#include <qemu-plugin.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "cdt_format.h"
#include "regmap.h"
#include "trace_writer.h"

QEMU_PLUGIN_EXPORT int qemu_plugin_version = QEMU_PLUGIN_VERSION;

#define MAX_DELTAS 256

struct shadow {
    uint64_t lo[REGMAP_MAX_SLOTS];
    uint64_t hi[REGMAP_MAX_SLOTS];
    bool valid;
};

static struct {
    struct trace_writer writer;
    struct regmap map;
    struct shadow shadow;
    GByteArray *scratch;
    uint64_t icount;
    uint64_t limit;
    bool is_started;
    bool is_finished;
    char feature_id[128];
    char qemu_version[32];
    enum regmap_scope scope;
} state;

static void emit_end(uint64_t reason)
{
    if (state.is_finished) {
        return;
    }
    state.is_finished = true;
    struct cdt_end_record record = {
        .hdr = { .type = CDT_REC_END, .flags = 0,
                 .length = sizeof(struct cdt_end_record) },
        .icount = state.icount,
        .reason = reason,
    };
    trace_writer_emit(&state.writer, &record, sizeof(record));
    trace_writer_close(&state.writer);
}

static uint64_t slot_value(unsigned index, uint64_t *hi)
{
    const struct regmap_slot *slot = &state.map.slots[index];
    uint64_t lo = 0, high = 0;
    if (!regmap_read(slot, state.scratch, &lo, &high)) {
        lo = 0;
        high = 0;
    }
    if (slot->reg_id == CDT_REG_PSTATE) {
        lo = regmap_normalise_pstate(lo);
    }
    if (hi) {
        *hi = high;
    }
    return lo;
}

/* Appends deltas for every slot whose value moved since the last block. */
static unsigned collect_deltas(struct cdt_reg_delta *out, unsigned capacity)
{
    unsigned count = 0;
    for (unsigned i = 0; i < state.map.n_block_slots && count + 1 < capacity; i++) {
        const struct regmap_slot *slot = &state.map.slots[i];
        uint64_t hi = 0;
        uint64_t lo = slot_value(i, &hi);
        bool is_new = !state.shadow.valid;
        if (is_new || lo != state.shadow.lo[i]) {
            out[count++] = (struct cdt_reg_delta){ .reg_id = slot->reg_id,
                                                   .value = lo };
        }
        if (slot->is_vector && (is_new || hi != state.shadow.hi[i])) {
            out[count++] = (struct cdt_reg_delta){
                .reg_id = (uint16_t)(slot->reg_id + 1), .value = hi };
        }
        state.shadow.lo[i] = lo;
        state.shadow.hi[i] = hi;
    }
    state.shadow.valid = true;
    return count;
}

struct block_info {
    uint64_t pc;
    uint16_t n_insns;
};

static void on_block_exec(unsigned int vcpu_index, void *udata)
{
    if (state.is_finished) {
        return;
    }
    const struct block_info *info = udata;

    uint8_t payload[sizeof(struct cdt_block_record) +
                    MAX_DELTAS * sizeof(struct cdt_reg_delta)];
    struct cdt_block_record *record = (struct cdt_block_record *)payload;
    struct cdt_reg_delta *deltas =
        (struct cdt_reg_delta *)(payload + sizeof(struct cdt_block_record));

    unsigned count = collect_deltas(deltas, MAX_DELTAS);
    size_t length = sizeof(struct cdt_block_record) +
                    count * sizeof(struct cdt_reg_delta);

    record->hdr = (struct cdt_record_header){ .type = CDT_REC_BLOCK,
                                              .flags = (uint8_t)count,
                                              .length = (uint16_t)length };
    record->pc = info->pc;
    record->icount = state.icount;
    record->n_insns = info->n_insns;
    memset(record->pad, 0, sizeof(record->pad));

    trace_writer_emit(&state.writer, payload, length);
    state.icount += info->n_insns;

    if (state.limit && state.icount >= state.limit) {
        emit_end(CDT_END_LIMIT);
    }
}

static void fill_exception_state(struct cdt_exception_record *record)
{
    for (unsigned n = 0; n < CDT_NUM_GPR; n++) {
        int index = state.map.gpr[n];
        record->x[n] = index >= 0 ? slot_value((unsigned)index, NULL) : 0;
    }
    record->x[31] = state.map.sp >= 0
                        ? slot_value((unsigned)state.map.sp, NULL) : 0;
    record->pc = state.map.pc >= 0
                     ? slot_value((unsigned)state.map.pc, NULL) : 0;
    record->pstate = state.map.pstate >= 0
                         ? slot_value((unsigned)state.map.pstate, NULL) : 0;
    record->fpcr = state.map.fpcr >= 0
                       ? slot_value((unsigned)state.map.fpcr, NULL) : 0;
    record->fpsr = state.map.fpsr >= 0
                       ? slot_value((unsigned)state.map.fpsr, NULL) : 0;
    for (unsigned n = 0; n < CDT_NUM_SYSREG; n++) {
        int index = state.map.sysreg[n];
        record->sysreg[n] = index >= 0 ? slot_value((unsigned)index, NULL) : 0;
    }
    for (unsigned n = 0; n < CDT_NUM_VREG; n++) {
        int index = state.map.vreg[n];
        uint64_t hi = 0;
        uint64_t lo = index >= 0 ? slot_value((unsigned)index, &hi) : 0;
        record->v[2 * n] = lo;
        record->v[2 * n + 1] = hi;
    }
}

static void on_discon(unsigned int vcpu_index, enum qemu_plugin_discon_type type,
                      uint64_t from_pc, uint64_t to_pc, void *udata)
{
    if (state.is_finished) {
        return;
    }
    struct cdt_exception_record record;
    memset(&record, 0, sizeof(record));
    record.hdr = (struct cdt_record_header){
        .type = CDT_REC_EXCEPTION, .flags = 0,
        .length = (uint16_t)sizeof(record) };
    record.icount = state.icount;
    record.from_pc = from_pc;
    record.to_pc = to_pc;
    record.discon_type = (uint32_t)type;
    fill_exception_state(&record);
    trace_writer_emit(&state.writer, &record, sizeof(record));

    /* An exception rewrites PC/PSTATE/ELR wholesale; force a full delta next
     * block so the two streams cannot silently drift out of sync. */
    state.shadow.valid = false;
}

static void on_tb_trans(struct qemu_plugin_tb *tb, void *udata)
{
    struct block_info *info = g_new0(struct block_info, 1);
    info->pc = qemu_plugin_tb_vaddr(tb);
    info->n_insns = (uint16_t)qemu_plugin_tb_n_insns(tb);
    qemu_plugin_register_vcpu_tb_exec_cb(tb, on_block_exec,
                                         QEMU_PLUGIN_CB_R_REGS, info);
}

static void emit_start_marker(void)
{
    struct cdt_marker_record record = {
        .hdr = { .type = CDT_REC_MARKER, .flags = 0,
                 .length = sizeof(struct cdt_marker_record) },
        .icount = 0,
        .kind = CDT_MARKER_TRACE_START,
        .value = 0,
    };
    trace_writer_emit(&state.writer, &record, sizeof(record));
}

static void on_vcpu_init(unsigned int vcpu_index, void *udata)
{
    if (state.is_started) {
        return;
    }
    state.is_started = true;
    regmap_build(&state.map, state.scope);
    state.scratch = g_byte_array_new();
    emit_start_marker();
}

static void on_exit(void *udata)
{
    emit_end(CDT_END_NORMAL);
}

static bool parse_args(int argc, char **argv, const char **out_path)
{
    for (int i = 0; i < argc; i++) {
        if (g_str_has_prefix(argv[i], "out=")) {
            *out_path = argv[i] + 4;
        } else if (g_str_has_prefix(argv[i], "limit=")) {
            state.limit = g_ascii_strtoull(argv[i] + 6, NULL, 0);
        } else if (g_str_has_prefix(argv[i], "cpu=")) {
            g_strlcpy(state.feature_id, argv[i] + 4, sizeof(state.feature_id));
        } else if (g_str_has_prefix(argv[i], "scope=")) {
            if (!regmap_parse_scope(argv[i] + 6, &state.scope)) {
                fprintf(stderr, "coracle-trace: bad scope '%s'"
                        " (expected core, fp or all)\n", argv[i] + 6);
                return false;
            }
        } else if (g_str_has_prefix(argv[i], "qemu=")) {
            g_strlcpy(state.qemu_version, argv[i] + 5,
                      sizeof(state.qemu_version));
        } else {
            fprintf(stderr, "coracle-trace: unknown argument '%s'\n", argv[i]);
            return false;
        }
    }
    return true;
}

static void build_header(struct cdt_file_header *header)
{
    memset(header, 0, sizeof(*header));
    memcpy(header->magic, CDT_MAGIC, 8);
    header->format_version = CDT_FORMAT_VERSION;
    header->producer = CDT_PRODUCER_QEMU;
    header->flags = CDT_FLAG_PRECISE_FP | CDT_FLAG_BLOCK_DELTAS;
    if (state.scope >= REGMAP_SCOPE_FP)  header->flags |= CDT_FLAG_HAS_VREGS;
    if (state.scope >= REGMAP_SCOPE_ALL) header->flags |= CDT_FLAG_HAS_SYSREGS;
    header->cpu_feature_id = cdt_fnv1a(state.feature_id);
    snprintf((char *)header->producer_name, sizeof(header->producer_name),
             "qemu-%s-%s-%s", state.qemu_version,
             regmap_scope_name(state.scope), state.feature_id);
}

QEMU_PLUGIN_EXPORT int qemu_plugin_install(qemu_plugin_id_t id,
                                           const qemu_info_t *info,
                                           int argc, char **argv)
{
    const char *path = "trace.cdt";
    g_strlcpy(state.feature_id, "unspecified", sizeof(state.feature_id));
    g_strlcpy(state.qemu_version, "unknown", sizeof(state.qemu_version));
    state.scope = REGMAP_SCOPE_ALL;
    if (!parse_args(argc, argv, &path)) {
        return -1;
    }

    struct cdt_file_header header;
    build_header(&header);
    if (!trace_writer_open(&state.writer, path, &header)) {
        return -1;
    }

    qemu_plugin_register_vcpu_init_cb(id, on_vcpu_init, NULL);
    qemu_plugin_register_vcpu_tb_trans_cb(id, on_tb_trans, NULL);
    qemu_plugin_register_vcpu_discon_cb(id, QEMU_PLUGIN_DISCON_ALL, on_discon,
                                        NULL);
    qemu_plugin_register_atexit_cb(id, on_exit, NULL);
    return 0;
}
