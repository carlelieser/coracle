# QEMU differential test harness

The correctness oracle for the emulator. QEMU runs a program and emits a
compact execution trace; the emulator emits the same format; a differ aligns
the two and reports the first divergence.

Satisfies the `docs/plan.md` M0 gate item: _"Differential harness runs a
10-instruction ELF and reports identical state."_

## Layout

| Path                    | What it is                                                            |
| ----------------------- | --------------------------------------------------------------------- |
| `TRACE_FORMAT.md`       | **The trace format.** Normative; other agents implement against this. |
| `EMULATOR_INTERFACE.md` | The hook the emulator must provide.                                   |
| `qemu-plugin/`          | The QEMU TCG plugin that emits traces.                                |
| `differ/`               | Trace reader, comparator, reporter, and the perturbation tool.        |
| `corpus/`               | Test programs (AArch64 assembly).                                     |
| `qemu_cpu.sh`           | **Single source of truth** for the QEMU pin and CPU feature mask.     |
| `run_tests.sh`          | The gate. Run this.                                                   |

## Prerequisites

macOS arm64 and Linux are both supported; no cross-toolchain and no container
are needed.

```sh
brew install qemu llvm       # macOS
```

- **QEMU 11.1.1**, pinned in `qemu_cpu.sh`. The Homebrew bottle ships
  `qemu-plugin.h`, so the plugin builds against the installed QEMU with no
  source tree. `run_qemu.sh` refuses to run against any other version — TCG
  behaviour and the plugin register list both move between releases.
- **clang** (Apple clang is fine) targets `aarch64-unknown-none` out of the
  box, so `corpus/*.s` assembles without a cross-toolchain.
- **llvm-objcopy** (from Homebrew `llvm`) extracts a flat image QEMU loads with
  `-kernel`. No linker is required.
- **Node 22+** for the differ. No npm dependencies.

On Linux, install `qemu-system-arm`, `qemu-system-common` (for the plugin
header), `clang`, `llvm`, and `node`; the Makefile and scripts detect the
platform.

## Running

```sh
./run_tests.sh                        # the M0 gate, end to end
./verify_feature_mask.sh              # assert the advertised CPU mask
./bench_overhead.sh 5000000           # emission overhead
```

Individual pieces:

```sh
make -C qemu-plugin
./build_corpus.sh
./run_qemu.sh build/m0_ten_insn.bin out/a.cdt 40
node differ/cdt.mjs dump  out/a.cdt
node differ/cdt.mjs stats out/a.cdt
node differ/diff.mjs out/a.cdt out/b.cdt
```

## Comparing the emulator against QEMU

```sh
./run_qemu.sh build/prog.bin out/qemu.cdt 1000000
<emulator> --trace-out out/coracle.cdt build/prog.bin
node differ/diff.mjs out/qemu.cdt out/coracle.cdt
```

Exit codes: `0` match, `1` divergence, `2` usage or format error.

## Testing the differ itself

A differ that has only ever seen matching traces is untested. `perturb.mjs`
injects a single-register fault into a real trace, preserving every offset:

```sh
node differ/perturb.mjs out/qemu.cdt out/bad.cdt --reg=x5 --at-step=2 --xor=0x40
node differ/diff.mjs out/qemu.cdt out/bad.cdt
```

`run_tests.sh` does this for a GPR, PSTATE, PC, and a system register inside an
exception record.

## Trace scope and emission cost

The plugin's `scope=` argument selects which registers are scanned on the
per-block hot path. **All scopes emit full architectural state at every
exception entry**, so the M2 gate is unaffected by this choice.

Measured on an M-series Mac, 5 M instructions of a branch-dense loop
(`corpus/bench_loop.s` — the pessimistic case, since cost is per block):

| scope           | reads/block | throughput | 50 M (M2) | 200 M (M5) |
| --------------- | ----------- | ---------- | --------- | ---------- |
| none (baseline) | 0           | 15.0 M/s   | 3 s       | 13 s       |
| `core`          | ~34         | 3.5 M/s    | 14 s      | 57 s       |
| `fp`            | ~68         | 2.0 M/s    | 25 s      | 100 s      |
| `all`           | ~140        | 1.0 M/s    | 50 s      | 200 s      |

Trace size is ~18.7 bytes/instruction in all scopes for this workload, so M5 is
~3.7 GB. Real boot code has larger basic blocks than this benchmark, so both
the time and the size are upper bounds.

`core` is the default for long runs; `all` is the default for correctness runs
where per-block system-register resolution is wanted.

## Known deviation: crypto

QEMU 11.1.1 offers **no property to disable FEAT_AES/SHA** on any AArch64 CPU
model — crypto is baked into every model's ID registers, and there is no
`aes=off`. The plan's mask excludes crypto, so the oracle advertises one
feature the emulator will not implement.

This is contained rather than ignored:

- `verify_feature_mask.sh` asserts crypto is the _only_ excluded feature QEMU
  still advertises, so the gap cannot silently widen.
- The M1 fuzz corpus draws from the plan's mask, so crypto encodings are out of
  the corpus by construction.
- Guest code selects code paths from `ID_AA64ISAR0_EL1`, which the **emulator**
  controls. Since the emulator will advertise no crypto, no guest takes a
  crypto path in a differential run.

The feature-mask string carries a `+qemucrypto` suffix so this is visible in
every trace header rather than buried in a comment.

## GDB stub

Deliberately not part of the comparison loop. GDB-stub stepping tops out near
10^4 steps/s, which is unusable at 50 M instructions (plan §2). It is retained
only for interactively narrowing a divergence the differ has already located:

```sh
qemu-system-aarch64 -M virt,gic-version=2 -cpu cortex-a53,aarch64=on,pmu=off \
    -m 128 -nographic -accel tcg -kernel build/prog.bin -S -gdb tcp::1234
```

Use the divergence report's `icount` and `pc` to place a breakpoint.
