//! wasm-bindgen bindings. The only crate the JS SDK links against.
//!
//! Skeleton only. The SDK surface (`Box.pull`, `vm.exec`, …) lands in M4/M6;
//! what exists now is the build-capability handshake the JS side needs in order
//! to pick between the threaded and degraded bundles.

extern crate alloc;

use coracle_core::guest_memory;
use coracle_core::threading;
use coracle_machine::MachineConfig;
use wasm_bindgen::prelude::*;

/// What this wasm bundle requires of the page, reported to JS at load time.
///
/// The SDK feature-detects `crossOriginIsolated` and `SharedArrayBuffer`, then
/// checks the bundle it loaded agrees. A mismatch is a load-time error rather
/// than a hang inside `Atomics.wait`.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub struct BuildInfo {
    threaded: bool,
    requires_cross_origin_isolation: bool,
    guest_ram_base: u32,
    default_guest_ram_bytes: u32,
    max_guest_ram_bytes: u32,
}

#[wasm_bindgen]
impl BuildInfo {
    /// Whether this bundle expects shared linear memory and a Web Worker.
    #[wasm_bindgen(getter)]
    pub fn threaded(&self) -> bool {
        self.threaded
    }

    /// Whether the page must be cross-origin isolated to run this bundle.
    #[wasm_bindgen(getter, js_name = requiresCrossOriginIsolation)]
    pub fn requires_cross_origin_isolation(&self) -> bool {
        self.requires_cross_origin_isolation
    }

    /// Byte offset of guest physical address 0 inside the exported memory.
    #[wasm_bindgen(getter, js_name = guestRamBase)]
    pub fn guest_ram_base(&self) -> u32 {
        self.guest_ram_base
    }

    /// Guest RAM in bytes when the embedder does not choose.
    #[wasm_bindgen(getter, js_name = defaultGuestRamBytes)]
    pub fn default_guest_ram_bytes(&self) -> u32 {
        self.default_guest_ram_bytes
    }

    /// Largest guest RAM this bundle can address.
    #[wasm_bindgen(getter, js_name = maxGuestRamBytes)]
    pub fn max_guest_ram_bytes(&self) -> u32 {
        self.max_guest_ram_bytes
    }
}

/// Reports how this bundle was built.
#[wasm_bindgen(js_name = buildInfo)]
pub fn build_info() -> BuildInfo {
    BuildInfo {
        threaded: coracle_core::IS_THREADED_BUILD,
        requires_cross_origin_isolation: threading::REQUIRES_CROSS_ORIGIN_ISOLATION,
        guest_ram_base: guest_memory::RAM_BASE as u32,
        default_guest_ram_bytes: guest_memory::DEFAULT_RAM_BYTES as u32,
        max_guest_ram_bytes: guest_memory::MAX_RAM_BYTES as u32,
    }
}

/// Validates a guest RAM size before the host allocates anything.
///
/// Returns the linear-memory byte offset guest RAM will start at.
#[wasm_bindgen(js_name = reserveGuestRam)]
pub fn reserve_guest_ram(ram_bytes: u32) -> Result<u32, JsError> {
    let config = MachineConfig {
        ram_bytes: ram_bytes as usize,
    };

    match config.validate() {
        Ok((start, _end)) => Ok(start as u32),
        Err(error) => Err(JsError::new(&alloc::format!(
            "reserve guest RAM of {ram_bytes} bytes: {error:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_reports_the_layout_the_js_host_must_agree_with() {
        let info = build_info();

        assert_eq!(info.guest_ram_base(), guest_memory::RAM_BASE as u32);
        assert_eq!(info.threaded(), coracle_core::IS_THREADED_BUILD);
        assert_eq!(
            info.requires_cross_origin_isolation(),
            coracle_core::IS_THREADED_BUILD
        );
    }

    #[test]
    fn the_reported_ram_window_matches_what_the_machine_will_reserve() {
        let info = build_info();
        let config = MachineConfig {
            ram_bytes: info.default_guest_ram_bytes() as usize,
        };

        let (start, end) = config.validate().expect("default RAM must reserve");
        assert_eq!(start as u32, info.guest_ram_base());
        assert_eq!((end - start) as u32, info.default_guest_ram_bytes());
    }
}
