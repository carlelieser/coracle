# arm64 Full-System Emulator in WebAssembly — Implementation Plan

## 1. Deliverable

A client-side engine that pulls any arm64 OCI image from a registry, boots it in a browser tab, and exposes it through a JS SDK and an interactive terminal. No server-side compute; the only backend is a stateless registry proxy.

```js
const vm = await Box.pull("node:22-alpine");
const { stdout } = await vm.exec(["node", "-e", "console.log(1+1)"]);
```

### Final acceptance criteria (all required)

| # | Criterion | Measure |
|---|-----------|---------|
| A1 | Runs unmodified official arm64 images | `alpine`, `debian`, `ubuntu`, `node:22-alpine`, `node:22`, `python:3.12` run their entrypoint |
| A2 | Boot latency from snapshot | < 3 s to a usable shell on a 2022 laptop, Chrome |
| A3 | Real workload | `npm install && npm test` on a 20-dependency Express project completes |
| A4 | Performance | Hot code ≤ 10× native; `node -e "1+1"` end-to-end < 500 ms after boot |
| A5 | Networking | Guest reaches an HTTPS endpoint through the tunnel; a server listening in the guest is reachable from the page via forwarded URL |
| A6 | Persistence | Root disk changes survive a page reload |
| A7 | Headless mode | Same engine runs under Node.js CLI with identical SDK |
| A8 | Client-only | Zero server compute; proxy is stateless and optional if registry allows CORS |

## 2. Fixed design decisions

These are decided now and not revisited without a written reason.

| Area | Decision | Why |
|------|----------|-----|
| Architecture | ARMv8.0-A, AArch64 only, EL0/EL1 only | Mainline kernel needs nothing more; EL2/EL3 are out of scope |
| Machine model | Clone of QEMU `virt`: GICv2, generic timer, PL011, virtio-mmio, device tree | Stock kernel boots unpatched; QEMU is the reference oracle |
| Firmware interface | Trap `SMC`/`HVC`; PSCI subset (`PSCI_VERSION`, `SYSTEM_OFF`, `SYSTEM_RESET`, `CPU_SUSPEND` as WFI); `psci` DTB node with `smc` conduit | Kernel on `virt` expects PSCI for boot, reboot, and poweroff; keeps EL3 out of scope |
| Feature mask | Advertise no LSE, SVE, PAuth, MTE, crypto, or BTI | Keeps ISA surface small; libc picks portable code paths |
| FP semantics | Native wasm FP while FPCR is in default mode (RN, no FZ, untrapped); softfloat fallback keyed on a cached FPCR flag; FPSR cumulative flags maintained only on the softfloat path; build-time "precise mode" runs softfloat everywhere for differential testing | wasm has no rounding-mode control or FP flags; default mode covers virtually all userspace, and the QEMU oracle stays usable |
| Language | Rust → wasm32 (`wasm-bindgen`), JS for glue only | Deterministic hot loop, no GC pauses |
| Guest RAM | Fixed-offset region inside the module's shared, exported linear memory (Rust built with atomics); default 1 GB, configurable, ≤ 2 GB; MMU translates to physical offsets | JIT-emitted modules import the same memory; full-system mode avoids 64-bit pointer masking; leaves tab-budget headroom for layer cache, JIT modules, and JS heap |
| Threads | CPU in a Web Worker; devices via SharedArrayBuffer; page is cross-origin isolated | Required for `Atomics.wait` and responsiveness |
| Execution | Basic-block interpreter with decoded-instruction cache; JIT slots in at M5 | Avoids rewrite when adding the translator |
| Page tracking | Dirty/executed page bitmaps in the MMU from M2 onward | Needed by snapshots (M5) and JIT invalidation (M5) |
| Storage | OPFS primary, IndexedDB fallback; content-addressed layer store | Large binary blobs, streaming reads |
| Root filesystem | Read-only layers merged by a JS overlay, exposed via virtio-9p; guest mounts overlayfs(lowerdir=9p, upperdir=ext4 on virtio-blk) — the kernel owns copy-up, whiteouts, rename | No ext4 image building; layers shared across images; writes land on real ext4 so xattrs, uid/gid, and hardlinks behave. Overlayfs-over-9p validated under QEMU at M0; JS-side union is the named fallback |
| Container runtime | Tiny guest-side `init` that reads image config and execs | No runc, no cgroups, no namespaces |
| Networking | Userspace TCP/IP stack in wasm (smoltcp or lwIP) bridged to a WebSocket relay; inbound: a service worker maps page-origin fetches onto guest TCP ports (relay protocol reserves reverse-tunnel frames) | Browsers have no raw sockets; previewing a guest dev server is a headline use case |
| Testing | Differential tests against QEMU `qemu-system-aarch64` via a TCG plugin emitting a compact execution trace (registers at block boundaries, full state at every exception entry); GDB stub only for narrowing a found divergence; FP legs run in precise mode | GDB-stub stepping tops out near 10^4 steps/s — unusable at 50 M instructions; a trace runs at near-full QEMU speed |
| Licensing | Engine Apache-2.0; hosted relay and image mirror are the commercial layer | Open core; avoids the CheerpX lock-in objection |

Non-goals for v1: AArch32, big-endian, GPU/framebuffer, USB, multi-core SMP (single vCPU), Windows or macOS guests, x86 images.

## 3. Milestones

Each milestone has a gate. Work on the next milestone does not start until the gate passes.

---

### M0 — Foundation

Deliverables
- Repo with Rust workspace: `core` (CPU/MMU), `devices`, `machine`, `wasm` (bindings), `js` (SDK, UI).
- CI running Rust unit tests natively and in `wasm32-unknown-unknown` via headless Chrome.
- QEMU differential test harness: a TCG plugin makes QEMU emit a compact binary trace (registers at basic-block boundaries, full state at every exception entry); the emulator emits the same format; a differ aligns and compares. GDB stub retained only for interactively narrowing a divergence. QEMU pinned and launched with `-cpu` flags matching the advertised feature mask.
- Kernel build script producing a pinned mainline kernel (`defconfig` + virtio + 9p + minimal drivers) and a busybox initramfs.
- Written machine spec: memory map, IRQ map, MMIO addresses, PSCI interface, device tree source.

Gate
- [ ] CI green on native and wasm targets.
- [ ] Differential harness runs a 10-instruction ELF and reports identical state.
- [ ] Kernel + initramfs boot to shell under QEMU `virt` using the same DTB the emulator will use.
- [ ] Rootfs spike under pure QEMU: kernel pivots to overlayfs(lowerdir=9p, upperdir=ext4 on virtio-blk) as root; copy-up, whiteout, rename, xattr, uid/gid, and hardlink behavior verified. If overlayfs-over-9p fails here, the fixed decision flips to the JS-side union fallback before M1 starts. Scope note: this spike uses QEMU's 9p server, so it settles only the kernel-side question (overlayfs accepts a 9p lower). Whether our JS 9p server provides the required fidelity (d_type, stable QIDs/inodes, xattrs) is a separate question, settled by the M3 metadata gates; the spike's output includes the exact 9p feature checklist those gates must cover.

---

### M1 — CPU core, user-mode harness

Scope
- A64 decoder covering: data processing (immediate, register, 3-source), loads/stores (all addressing modes, pairs, exclusives, acquire/release), branches, system instructions, conditional select/compare, bit manipulation.
- FP: scalar SP/DP arithmetic, conversions, compares, FMA. Two backends behind one trait: native wasm ops (used while FPCR is in default mode) and softfloat (non-default FPCR, and precise mode for differential runs). FPSR cumulative flags exist only on the softfloat path.
- NEON: the subset used by musl/glibc `memcpy`, `memset`, `strlen`, `memchr`, and by V8's baseline. Implement lazily, driven by a trap-and-log on unimplemented opcode.
- Exception model at EL0 level: SVC, undefined, alignment, and data abort delivery to a pluggable handler.
- Throwaway Linux syscall shim (~40 syscalls) so static binaries run without a kernel. This shim is deleted after M2; it exists only to test the CPU.

Deliverables
- `core` crate with decoder, interpreter, register file, FP/NEON.
- Instruction coverage report generated from the test corpus.
- Benchmark: Dhrystone and a CoreMark-like loop, native vs interpreted.

Gate
- [ ] Differential test: 100% register-state match against QEMU on a 10,000-binary fuzz corpus (random A64 instructions drawn from the advertised feature mask; unallocated encodings must fault identically) and on the full musl test suite compiled static. FP legs run in precise mode; the native FP backend passes a separate default-mode equivalence suite that ignores NaN payload bits.
- [ ] Static `busybox` runs `sh -c 'echo hi | wc -c'` correctly under the shim.
- [ ] Exclusive-monitor (`LDXR`/`STXR` loops) and TLS (`TPIDR_EL0`) corpus passes. The shim stays single-threaded — `clone`/`futex` are explicitly out of shim scope; real threads arrive with the kernel at M2, and the `node` smoke test lands at M3.
- [ ] Interpreter ≤ 60× slower than native on the benchmark, and ≥ 40 guest MIPS absolute on a boot-profile workload (the absolute number, not the ratio, is what predicts the M2 60 s boot bound).

---

### M2 — System mode: boot Linux to a shell

Scope
- EL1: system register file (SCTLR, TCR, TTBR0/1, MAIR, VBAR, ESR, FAR, ELR, SPSR, DAIF, CNT*, ID_AA64*), exception entry/return, `ERET`, `MSR/MRS`.
- Stage-1 MMU: 4 KB granule, 48-bit VA, page-table walk, software TLB with ASID, `TLBI` variants, access-flag and permission faults, `DC`/`IC` cache ops as no-ops with correct invalidation of the decoded-instruction cache.
- Exclusive monitor semantics for `LDXR/STXR`.
- GICv2 distributor and CPU interface; generic timer (virtual and physical); PL011 UART with an xterm.js console; PL031 RTC.
- PSCI via trapped `SMC`: `SYSTEM_OFF`/`SYSTEM_RESET` terminate or restart the machine loop; `CPU_SUSPEND` idles until the next timer or IRQ.
- Device tree generation matching the M0 spec.
- Dirty and executed page bitmaps maintained by the MMU.
- Machine runs in a Web Worker; console I/O bridged to the main thread.

Deliverables
- `machine` crate booting the M0 kernel and initramfs.
- Browser page with a terminal that reaches a busybox shell.
- Boot-time profile identifying the top 20 hottest guest functions.

Gate
- [ ] Kernel boots to a login-free shell with `console=ttyAMA0`, no kernel warnings or oopses in `dmesg`.
- [ ] `dmesg` timer calibration within 5% of wall clock; `sleep 5` takes 5 s ± 0.5 s.
- [ ] 1,000 consecutive `fork`+`exec` in a shell loop with no hang or corruption.
- [ ] `poweroff` and `reboot` work via PSCI (clean machine-loop exit and restart).
- [ ] Differential test against QEMU (TCG-plugin trace) on the first 50 million instructions of boot: identical register and system-register state at every exception entry.
- [ ] Cold boot to shell ≤ 60 s in Chrome on the reference laptop. (Snapshots come later; this bounds interpreter quality.)

This is the go/no-go milestone. If M2 cannot be met, stop and reassess the approach.

---

### M3 — virtio devices and persistence

Scope
- virtio-mmio transport, legacy and modern.
- virtio-blk: backed by a chunked sparse disk in OPFS (64 KB chunks, lazy fetch from a URL, copy-on-write upper layer).
- Root wiring per the fixed decision: initramfs mounts overlayfs(lowerdir=9p merged layers, upperdir=ext4 on virtio-blk) and pivots to it.
- virtio-9p (or virtiofs if simpler to keep correct): exposes JS-side directory trees to the guest, read-write, with full metadata fidelity — uid/gid, mode bits incl. setuid, symlinks, hardlinks, xattrs, device nodes — sufficient to serve as an overlayfs lower layer.
- virtio-net: guest frames go to a wasm TCP/IP stack (smoltcp or lwIP) which multiplexes TCP/UDP/DNS over a single WebSocket to a relay. Relay is a 200-line stateless Node program.
- virtio-rng, virtio-console (secondary).

Deliverables
- `devices` crate with all listed virtio devices.
- Relay server in the repo with Docker image.
- Persistent root disk demo: write a file, reload page, file is still there.

Gate
- [ ] Kernel pivots to the overlayfs root (9p lower, ext4-on-blk upper) and boots from it instead of initramfs.
- [ ] `node` on the overlayfs root prints `console.log(1+1)`.
- [ ] `fio`-style sequential read ≥ 20 MB/s from a cached disk; random 4 KB read ≥ 2,000 IOPS.
- [ ] 9p: `tar -xf` of a 10,000-file archive into a 9p mount and back produces a byte-identical archive.
- [ ] Our 9p server passes the M0 spike's feature checklist as an overlayfs lower (d_type, stable QIDs, xattrs, hardlinks) — same overlayfs test matrix, our server instead of QEMU's.
- [ ] 9p metadata rate ≥ 2,000 ops/s (stat+open+close loop over cached small files) — the small-file floor under `npm install` (A3).
- [ ] `curl https://example.com` succeeds through the relay; `apk add curl` succeeds against a public mirror.
- [ ] Root disk state survives 10 reload cycles with `fsck` clean.

---

### M4 — OCI image pipeline

Scope
- Registry client in JS: token auth, manifest list resolution to `linux/arm64`, blob fetch with range requests and resume, content-addressed cache in OPFS.
- Stateless CORS proxy for registries that block browser origins (Docker Hub, GHCR). Proxy forwards headers and bytes only; no storage.
- Layer unpacking: tar streams parsed in JS or wasm into the layer store; whiteouts handled.
- JS-side overlay merging read-only layers into a single lower tree served over 9p (whiteouts applied at merge time); the guest's overlayfs supplies the writable upper on virtio-blk.
- Guest `init`/agent (static, < 200 KB): mounts the overlay root, proc, sys, dev; applies image config (`Entrypoint`, `Cmd`, `Env`, `WorkingDir`, `User`); reaps children. Speaks a framed control protocol over two virtio-console channels (control + data): concurrent `exec` sessions, PTY allocation and resize, signal delivery, stdout/stderr demux, per-session exit codes. This is the guest half of `vm.exec`/`vm.spawn`.
- Prebuilt kernel + initramfs is the same for every image; only the 9p root changes.

Deliverables
- `Box.pull(ref)` and `Box.run()` in the SDK.
- Image browser UI: enter a ref, see layer download progress, get a terminal.
- Proxy Docker image.

Gate
- [ ] All A1 images pull and run their entrypoint with correct env and working directory.
- [ ] Pulling `node:22-alpine` cold completes in < 30 s on 50 Mbit/s; second pull of a different Alpine-based image reuses the shared base layer (verified by zero refetch of that blob).
- [ ] Whiteout test image (file deleted in an upper layer) shows the file absent in the guest.
- [ ] `python:3.12` runs `python -c "import ssl, sqlite3, json; print('ok')"`.
- [ ] Exit status of the entrypoint is returned to `Box.run()` correctly for codes 0, 1, and 137.

---

### M5 — Snapshots and JIT

Scope, part A: snapshots
- Serialize full machine state: CPU, system registers, TLB, device state, dirty RAM pages (compressed), disk COW map.
- Restore into a fresh worker; devices reconnect (WebSocket, OPFS handles). Restore reseeds guest entropy — fresh virtio-rng pool plus an injected reseed of the kernel CRNG — so a published snapshot doesn't hand every user identical randomness.
- Pre-booted snapshot published per kernel version; per-image snapshots taken after `init` reaches the entrypoint.

Gate A
- [ ] Restore-to-shell < 3 s from a warm cache; < 8 s including snapshot download on 50 Mbit/s.
- [ ] 100 snapshot/restore cycles under a running `node` HTTP server; server keeps serving after each restore.
- [ ] Snapshot of a booted Alpine ≤ 25 MB compressed.

Scope, part B: dynamic binary translator
- Hot-block detection with execution counters in the decoded-instruction cache.
- Translator emits WASM bytecode for a basic block (or extended block up to 64 instructions); blocks compiled in batches of ≥ 32 into one module to amortize `WebAssembly.instantiate`; functions placed in a shared `WebAssembly.Table`; block chaining via table index.
- Guest memory accessed through the TLB fast path inlined in generated code; slow path calls back into Rust.
- Invalidation: the MMU's executed-page bitmap plus write-protection traps evict translations on write, covering V8's runtime code generation.
- Correctness mode: run translated and interpreted execution in lockstep on a test corpus.

Gate B
- [ ] Lockstep mode shows zero divergence on the full M1 differential corpus and on a 200-million-instruction boot trace (FP compared under the M1 policy: precise mode for softfloat legs; NaN-payload-insensitive for the native backend).
- [ ] Node `v8-bench`-style workload (`octane` subset) ≤ 10× native; Dhrystone ≤ 5× native.
- [ ] Self-modifying-code test: guest program writes new code and executes it 100,000 times; correct results, no crash.
- [ ] Node `npm install && npm test` on the A3 project completes in < 5× the native wall time.
- [ ] No memory growth over 1 hour of a `node` HTTP server under load (translation cache bounded and evicting).

---

### M6 — SDK, headless mode, product surface

Scope
- SDK API frozen at 1.0: `Box.pull`, `Box.fromSnapshot`, `vm.exec`, `vm.spawn` (streaming), `vm.fs.read/write/list`, `vm.snapshot`, `vm.network.connect`, `vm.on('exit' | 'stdout' | 'stderr')`. Shape mirrors E2B so agent frameworks can swap backends.
- Headless host: the same wasm engine under Node.js with `worker_threads`, local disk-backed storage, and direct TCP instead of the relay.
- Inbound port forwarding: `vm.network.expose(port)` returns a URL; a service worker intercepts fetches to it and maps them onto guest TCP ports through the netstack. Headless mode binds a local port directly.
- Terminal React component and vanilla web component.
- Documentation site with runnable examples; hosted demo; relay and image-mirror deployment guides.
- Telemetry off by default.

Gate (release)
- [ ] All A1–A8 acceptance criteria pass on Chrome, Firefox, and Safari (desktop) and under Node 22 headless.
- [ ] Inbound: `python -m http.server 8000` in the guest is served to the page through the forwarded URL; same via a local port in headless mode.
- [ ] SDK test suite: 100% of public API covered, runs in both browser and headless.
- [ ] Two external users run the A3 workload from the docs alone without assistance.
- [ ] Security review of the relay (no open proxy; per-session auth) and the registry proxy (allowlisted hosts only).

## 4. Risk register

| Risk | Impact | Mitigation | Milestone |
|------|--------|------------|-----------|
| Guest JIT (V8) writes code the translator already compiled | Crashes or interpreter-speed Node | Executed-page bitmap and write traps built in M2; lockstep test in M5 | M2, M5 |
| NEON coverage gaps | Random SIGILL in libc or V8 | Trap-and-log unimplemented opcodes; coverage report; differential fuzz | M1 |
| Interpreter too slow to boot in reasonable time | M2 gate fails | Decoded-instruction cache from day one; profile-guided fast paths; 60 s bound is generous | M2 |
| `WebAssembly.instantiate` latency stalls the JIT | Jank on hot loops | Batch compilation; interpreter keeps running until module is ready | M5 |
| Registry CORS and rate limits | Images cannot be pulled | Stateless proxy; content-addressed cache; optional mirror | M4 |
| Browser storage quotas | Images evicted | OPFS with `persist()` request; LRU eviction of layers; user-visible usage | M3, M4 |
| Safari SharedArrayBuffer and OPFS quirks | One browser broken | Browser matrix in CI from M2; single-threaded degraded mode is a first-class, feature-detected build kept green in CI | M2+ |
| Timer accuracy under throttled background tabs | Guest clock drifts | Resync from host clock on visibility change; virtual time when hidden | M2 |
| COOP/COEP: embedding pages must be cross-origin isolated for SAB | SDK unusable on customer sites that can't adopt the headers | Document the requirement; single-threaded degraded mode as fallback; hosted-iframe option | M6 |
| overlayfs-over-9p quirks (d_type, rename, xattr) | Root filesystem redesign | Validated under pure QEMU in the M0 gate, before any emulator work depends on it; JS-side union is the named fallback | M0 |
| Native-FP divergence from ARM semantics (FPSR flags, NaN payloads, non-default FPCR) | Subtle userspace breakage; differential-test noise | Softfloat fallback keyed on FPCR; precise mode for all differential runs; FPSR flags only on the softfloat path, documented | M1, M5 |
| Scope creep into SMP, AArch32, GPU | Milestone slip | Explicit non-goals list; changes require written justification | All |

## 5. Milestone summary

| Milestone | Gate in one line |
|-----------|------------------|
| M0 Foundation | CI, QEMU harness, kernel boots under QEMU |
| M1 CPU core | 100% differential match; busybox under shim; ≥ 40 guest MIPS |
| M2 Boot Linux | Shell in the browser, clean `dmesg`, ≤ 60 s cold boot — go/no-go |
| M3 virtio | Persistent root disk, 9p, network through relay, `node` runs |
| M4 OCI images | All acceptance images run their entrypoint |
| M5 Snapshots + JIT | < 3 s restore; `npm test` ≤ 5× native |
| M6 SDK + release | A1–A8 incl. inbound forwarding pass on three browsers and headless |

