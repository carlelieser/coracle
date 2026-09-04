//! Tests that must run in a real browser: anything crossing the JS boundary.
//!
//! Run with `wasm-pack test --headless --chrome crates/wasm`, or via the
//! `wasm-test` / `wasm-test-st` CI jobs.

#![cfg(target_arch = "wasm32")]

use coracle_wasm::{build_info, reserve_guest_ram};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn the_bundle_reports_the_threading_mode_it_was_compiled_for() {
    let info = build_info();

    // The SDK picks a bundle from `crossOriginIsolated`; if this disagreed with
    // how the crate was compiled, the worker would hang in `Atomics.wait`.
    assert_eq!(info.threaded(), cfg!(feature = "threads"));
    assert_eq!(info.requires_cross_origin_isolation(), info.threaded());
}

#[wasm_bindgen_test]
fn guest_ram_sits_at_a_fixed_offset_inside_the_exported_memory() {
    let info = build_info();

    assert_eq!(info.guest_ram_base(), 1 << 30);
    assert_eq!(info.default_guest_ram_bytes(), 1 << 30);
    assert!(info.max_guest_ram_bytes() >= info.default_guest_ram_bytes());
}

#[wasm_bindgen_test]
fn reserving_valid_guest_ram_returns_the_fixed_base() {
    let base = reserve_guest_ram(1 << 30).expect("1 GiB is a valid guest RAM size");

    assert_eq!(base, build_info().guest_ram_base());
}

#[wasm_bindgen_test]
fn an_unaligned_guest_ram_size_surfaces_a_js_error() {
    // The failure has to reach JS as an exception, not as a silent clamp.
    assert!(reserve_guest_ram(1234).is_err());
}

#[wasm_bindgen_test]
fn atomics_are_available_exactly_when_this_is_a_threaded_build() {
    // Proves the target features in .cargo/config.toml actually reached the
    // module, rather than the build silently falling back to a non-atomics
    // sysroot. `compare_exchange` on a shared value lowers to an atomic
    // instruction only when +atomics is on.
    let is_atomics_build = cfg!(target_feature = "atomics");

    assert_eq!(is_atomics_build, cfg!(feature = "threads"));
}
