# What the emulator must implement to be differentially testable

This is the contract between the emulator and the harness. It is a *hook*
specification — the harness does not build any of it, and nothing here requires
changes to the emulator's own architecture.

## 1. Emit a CDT stream

Add a build-time-gated tracer that writes `tests/TRACE_FORMAT.md` v1. Suggested
Rust shape, in whatever crate owns the machine loop:

```rust
pub trait TraceSink {
    /// Called after every basic block retires. `deltas` carries only the
    /// registers this block changed.
    fn on_block(&mut self, pc: u64, icount: u64, n_insns: u16,
                deltas: &[(u16, u64)]);

    /// Called on every exception, interrupt and PSCI/HVC/SMC call, AFTER the
    /// CPU has been redirected to the vector.
    fn on_exception(&mut self, event: &ExceptionEvent);

    fn finish(&mut self, reason: EndReason);
}
```

Gate it behind a Cargo feature (e.g. `trace`) so release builds carry no cost.

### Required call ordering

1. `REC_MARKER` (kind 1) once, before the first block.
2. `on_block` for every retired block, in execution order.
3. `on_exception` *between* the block that faulted and the block at the vector.
4. `finish` exactly once.

## 2. Semantics that must match, exactly

These are the places where a plausible implementation choice would produce a
false divergence. The QEMU plugin already behaves as described.

| Field | Rule |
|-------|------|
| `icount` | Instructions retired **before** this block. Starts at 0. A block of `n` instructions advances it by `n`. |
| `icount` on exception | The value as of the faulting instruction — i.e. the faulting instruction is **not** counted as retired. |
| `pc` in `REC_BLOCK` | Virtual address of the **first** instruction of the block. |
| `n_insns` | Instructions in the block as translated, including the terminating branch. |
| Block boundaries | Must match QEMU's TCG blocks. See §4 — this is the one genuinely hard requirement. |
| `pstate` | Normalised per TRACE_FORMAT §6: NZCV at 31..28, DAIF at 9..6, EL at 3..2, SPSel at 0, **all other bits zero**. |
| `xzr` | Never emitted. Register id 31 is `sp`. |
| Deltas | Emit a register **only** when its value changed since the previous record. Emitting unchanged registers is harmless for correctness but inflates the trace. |
| After an exception | Treat the next block as if no shadow state is known and emit a full register set. The QEMU plugin does this so the two streams cannot silently drift. |

## 3. Feature-mask identity

The emulator must write the same `cpu_feature_id` the harness uses, or the
differ refuses to compare. It is `FNV-1a` (64-bit, offset basis
`0xcbf29ce484222325`, prime `0x100000001b3`) over the ASCII string in
`tests/qemu_cpu.sh` as `CORACLE_FEATURE_ID`:

```
armv8.0-a+el1+nolse+nosve+nopauth+nomte+nobti+qemucrypto
```

Read it from that file rather than hardcoding it; it changes if the mask does.

## 4. Block-boundary agreement — the one real constraint

`REC_BLOCK` records only line up if both producers cut blocks at the same
places. QEMU ends a TCG block on any branch, on any instruction that changes
control flow or system state it cannot chain across, and **at page boundaries**.
An interpreter that cuts blocks differently will diverge on `pc`/`n_insns`
immediately, even when its architectural state is perfect.

Two ways out, both supported by the format:

- **Preferred:** the emulator ends a block wherever QEMU does. Straightforward
  for branches; the page-boundary rule needs deliberate implementation.
- **Fallback:** emit one `REC_BLOCK` per instruction (`n_insns = 1`). The
  format allows it, and the differ needs no changes — QEMU's side can be made
  to match by running the plugin with per-instruction callbacks. This costs
  roughly 3x trace size and emission time, so it is the M1 debugging mode
  rather than the M2/M5 default.

The M2 gate ("identical state at every exception entry") does **not** depend on
block alignment — only `REC_EXCEPTION` records matter there. If block alignment
proves expensive, M2 can be gated on exceptions alone by filtering both streams
to `REC_EXCEPTION`; the differ already compares those independently.

## 5. FP

Differential legs must set `CDT_FLAG_PRECISE_FP` and run softfloat everywhere,
per plan §2. The native-wasm backend leg clears that flag; the differ then
defaults to the NaN-payload-insensitive policy. Both legs always write full bit
patterns — the policy lives in the differ, never in the producer.

## 6. What the emulator does NOT need to provide

- Memory access records. Not in v1 (TRACE_FORMAT §8).
- TLB or cache state. Not architecturally observable.
- Any GDB integration. The GDB stub path is for interactive narrowing only.
