# Coracle differential trace format (CDT) v1

Status: **stable interface**. QEMU (via TCG plugin) and the emulator both emit
this. The differ consumes two streams and reports the first divergence.

All integers are **little-endian**. All records are **8-byte aligned** and
self-describing in length, so a reader can skip a record type it does not know.

---

## 1. Design constraints

The format is driven by three numbers from `docs/plan.md`:

| Gate | Volume |
|------|--------|
| M0 | 10 instructions |
| M2 | 50 M instructions of boot, compared at every exception entry |
| M5 | 200 M instructions, lockstep |

At 200 M instructions a naive "full state per block" trace (~1 KB × 50 M blocks)
is ~50 GB. That is not workable. Two decisions bring it down:

1. **Block records carry a register delta, not full state.** A basic block
   typically writes 1–3 registers. A delta record is 16–40 bytes instead of
   ~1 KB.
2. **Full architectural state appears only at exception entry**, which is what
   the M2 gate actually compares. Boot takes O(10^5) exceptions over 50 M
   instructions, so full-state records are a rounding error in the total.

Measured M5 volume: **~3.7 GB** for 200 M instructions (18.7 bytes/instruction
on a deliberately branch-dense benchmark, which is the pessimistic case since
cost is per block). Streamable and compressible; see `README.md` for the
measured emission rates per scope.

## 2. Stream layout

```
FileHeader
Record*
```

Records are a tagged union. Every record begins with a common 8-byte prologue:

```c
struct RecordHeader {
    uint8_t  type;      // enum RecordType
    uint8_t  flags;     // type-specific
    uint16_t length;    // total record bytes incl. this header, multiple of 8
    uint32_t reserved;  // 0
};
```

`length` being present on *every* record is deliberate: a consumer that does not
understand a record type can still advance. This is the forward-compatibility
hook for M1/M2/M5 additions (memory records, TLB records) without a format
rewrite.

## 3. File header

```c
struct FileHeader {
    uint8_t  magic[8];        // "CORACLE\x01"
    uint32_t format_version;  // 1
    uint32_t producer;        // 1 = qemu-tcg-plugin, 2 = coracle emulator
    uint64_t flags;           // see StreamFlags
    uint64_t cpu_feature_id;  // FNV-1a of the advertised feature mask string
    uint8_t  producer_name[32]; // NUL-padded, e.g. "qemu-11.1.1-cortex-a53"
    uint64_t reserved[2];
};                            // 80 bytes
```

`cpu_feature_id` is a guard, not decoration. If the two streams disagree on it
the differ **refuses to run** rather than reporting thousands of spurious
divergences. This directly addresses the "QEMU advertises features we don't
implement" failure mode.

### StreamFlags

| Bit | Name | Meaning |
|-----|------|---------|
| 0 | `PRECISE_FP` | Producer ran softfloat everywhere (plan §2 "precise mode") |
| 1 | `HAS_VREGS` | Stream includes V-register state |
| 2 | `HAS_SYSREGS` | Stream includes EL1 system registers |
| 3 | `BLOCK_DELTAS` | Block records are deltas (always set in v1) |

`HAS_VREGS` and `HAS_SYSREGS` describe the **block** records only. Exception
records always carry the full state arrays regardless of these flags; the flags
say whether per-block deltas for those registers can be expected.

## 4. Record types

```c
enum RecordType {
    REC_BLOCK      = 1,  // basic block retired, with register delta
    REC_EXCEPTION  = 2,  // exception/interrupt entry, full architectural state
    REC_MARKER     = 3,  // synchronisation / annotation
    REC_END        = 4,  // clean end of stream
};
```

### 4.1 REC_BLOCK — the hot path

```c
struct BlockRecord {
    RecordHeader hdr;      // type=1, flags=n_deltas
    uint64_t     pc;       // virtual address of block start
    uint64_t     icount;   // cumulative instructions retired BEFORE this block
    uint16_t     n_insns;  // instructions in this block
    uint16_t     pad[3];
    RegDelta     deltas[]; // flags many
};

struct RegDelta {
    uint16_t reg_id;       // see §5
    uint16_t pad;
    uint32_t pad2;
    uint64_t value;        // lo 64 bits; V-regs emit two deltas (lo, hi)
};                         // 16 bytes
```

`icount` is the **alignment key**. The differ does not try to match on PC alone,
because a divergence in a branch makes PC sequences drift apart immediately;
`icount` gives a monotonic axis both producers agree on.

A block whose delta set is empty still emits a record — the PC/icount pair is
itself the observable being compared, and a control-flow divergence with no
register change (a mispredicted conditional branch) must still be caught.

### 4.2 REC_EXCEPTION — full state

Emitted on every exception, interrupt, and host call. This is what the M2 gate
compares.

```c
struct ExceptionRecord {
    RecordHeader hdr;        // type=2
    uint64_t     icount;
    uint64_t     from_pc;    // faulting / interrupted instruction
    uint64_t     to_pc;      // vector entry the CPU moved to
    uint32_t     discon_type;// 1=interrupt, 2=exception, 4=hostcall
    uint32_t     pad;
    uint64_t     x[32];      // x0..x30, then SP
    uint64_t     pc;
    uint64_t     pstate;     // NZCV/DAIF/EL/SPSel, normalised (§6)
    uint64_t     sysreg[N];  // §5.3 fixed order
    uint64_t     fpcr, fpsr;
    uint64_t     v[64];      // 32 × 128-bit as (lo, hi) pairs
};
```

Field order is fixed and dense — no per-field ids — because these records are
rare and the differ wants to compare them field-by-field with a stable name
table.

### 4.3 REC_MARKER

```c
struct MarkerRecord {
    RecordHeader hdr;   // type=3
    uint64_t     icount;
    uint64_t     kind;  // 1=trace start, 2=reset, 3=user annotation
    uint64_t     value;
};
```

### 4.4 REC_END

```c
struct EndRecord {
    RecordHeader hdr;       // type=4
    uint64_t     icount;    // total instructions retired
    uint64_t     reason;    // 0=normal, 1=limit reached, 2=guest halt
};
```

## 5. Register identifiers

Stable numeric ids, **never renumbered**. This is a wire contract.

| Range | Meaning |
|-------|---------|
| 0–30 | `x0`–`x30` |
| 31 | `sp` |
| 32 | `pc` |
| 33 | `pstate` (normalised, §6) |
| 34 | `fpcr` |
| 35 | `fpsr` |
| 64–127 | V registers, `64 + 2*n` = `vn` low 64 bits, `+1` = high |
| 256+ | system registers, §5.3 |

### 5.3 System registers (M2+)

Fixed order, ids 256 upward:

```
256 SCTLR_EL1   257 TTBR0_EL1   258 TTBR1_EL1   259 TCR_EL1
260 MAIR_EL1    261 VBAR_EL1    262 ESR_EL1     263 FAR_EL1
264 ELR_EL1     265 SPSR_EL1    266 SP_EL0      267 SP_EL1
268 TPIDR_EL0   269 TPIDR_EL1   270 TPIDRRO_EL0 271 CONTEXTIDR_EL1
272 CPACR_EL1   273 AMAIR_EL1   274 PAR_EL1     275 CNTKCTL_EL1
```

M0/M1 streams set `HAS_SYSREGS = 0` and omit these; the differ then compares
only the architectural core, which is the correct scope for a user-mode gate.

## 6. Normalisation rules

Bit-exact comparison of raw QEMU state produces false divergences. The producer
normalises before writing:

1. **PSTATE** is packed into a canonical layout, not QEMU's internal `cpsr`:

   ```
   bit 31..28  NZCV
   bit 9..6    DAIF
   bit 3..2    CurrentEL
   bit 0       SPSel
   ```

   All other bits **must be zero**. QEMU's `cpsr` carries AArch32 residue and
   internal flags that mean nothing to us.

2. **`x31`** is never emitted. `xzr` reads as zero by definition; `sp` is id 31.

3. **Unused V-register halves** are zeroed, not left as stale lanes.

## 7. FP comparison policy

Per `docs/plan.md` §2 and the M1 gate, FP is **not** always compared bitwise.
The policy is a property of the *comparison*, not of the trace, so both
producers always write full bit patterns and the differ decides.

| Policy | Rule |
|--------|------|
| `bitwise` | Exact equality. Used when both streams set `PRECISE_FP`. |
| `nan-insensitive` | Two values are equal if bitwise equal, **or** if both are NaN of the same signalling class. Used for the native-wasm FP backend leg. |
| `ignore-fpsr` | As above, and FPSR cumulative bits (IDC/IXC/UFC/OFC/DZC/IOC) are not compared. The plan states FPSR flags are only maintained on the softfloat path. |

The differ selects the policy from a CLI flag; `PRECISE_FP` in both headers
makes `bitwise` the default. Choosing `nan-insensitive` when both streams are
precise is allowed but warned about, since it weakens a gate that could be
strict.

## 8. What is deliberately absent in v1

Recorded so the omission is a decision, not an oversight:

- **Memory values.** A store that diverges shows up as a divergent load result
  in a later block delta, one block later. Logging every access would multiply
  trace size by ~10× and is not needed to satisfy any stated gate. `length`-
  prefixed records leave room to add `REC_MEM` at M3 if disk/9p work needs it.
- **TLB and cache state.** Not architecturally observable.
- **Per-instruction records.** Block granularity is what the plan specifies.
  A `REC_BLOCK` with `n_insns = 1` degenerates to per-instruction if a future
  narrowing pass wants it, so the format does not block that.
