# coracle

An arm64 full-system emulator targeting WebAssembly. See `docs/plan.md` for the
specification and milestones, and `docs/status.md` for what is built against
them.

Status: M1. The A64 decoder and the syscall shim are in place; the interpreter
executes 8 opcodes, so no M1 gate is met yet.

## Layout

| Path             | Contents                                                  |
| ---------------- | --------------------------------------------------------- |
| `crates/core`    | AArch64 CPU, stage-1 MMU, guest memory layout             |
| `crates/devices` | virtio-mmio, GICv2, generic timer, PL011, PL031           |
| `crates/machine` | QEMU `virt` machine model: wiring, device tree, main loop |
| `crates/wasm`    | wasm-bindgen bindings; the only crate the SDK links       |
| `js`             | SDK and UI                                                |

Dependencies run one way — `core` ← `devices` ← `machine` ← `wasm` — and Cargo
rejects a cycle, so the direction is enforced rather than agreed.

## Toolchain

Pinned by `rust-toolchain.toml`; `rustup` installs it on first use. The pin is a
nightly because the threaded wasm build needs `-Z build-std`, which is not on
stable. Node 22+ is required for the JS package.

Browser tests need a `chromedriver` matching the installed Chrome.
`scripts/chromedriver.sh` fetches and caches one, and prints its path. Without
it wasm-bindgen falls back to Safari and fails with an opaque driver error.

## Building and testing

```sh
cargo test --workspace   # native
npm ci && npm test       # js
export CHROMEDRIVER="$(scripts/chromedriver.sh)"
cargo wasm-test          # browser, threaded build
cargo wasm-test-st       # browser, single-threaded build
cargo fmt --all && npm run format   # rustfmt for Rust, Prettier for the rest
```

`cargo wasm` and `cargo wasm-st` build the two variants without testing.

## The two wasm builds

Both ship, both are required to stay green in CI.

**Threaded** (`cargo wasm`) is the default: the CPU runs in a Web Worker against
shared linear memory, and devices reach it over a `SharedArrayBuffer`. The page
must be cross-origin isolated:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

`wasm32-unknown-unknown` ships a std built without the atomics feature, and
atomics is not ABI-compatible with code that lacks it, so this variant rebuilds
the sysroot from source with `-Z build-std`. Its linker flags are in
`.cargo/config.toml`, on the aliases rather than in a `[target.*]` table —
deliberately, because cargo takes `rustflags` from a single source and a table
there would make the non-atomics variant unbuildable. For the same reason, do not
set `RUSTFLAGS` in the environment when building: it replaces the alias flags
instead of adding to them, and the result is a threaded build with no atomics.

**Single-threaded** (`cargo wasm-st`) is the degraded build for Safari and for
pages that cannot set those headers. It uses the stock sysroot, has no atomics,
and yields back to the host instead of blocking on `Atomics.wait`. The SDK
feature-detects `crossOriginIsolated` and loads whichever build applies.

Guest RAM is a fixed-offset region inside the module's exported linear memory
rather than a heap allocation, because M5's JIT modules import the same memory
and need the displacement to be a link-time constant. See
`crates/core/src/guest_memory.rs`.
