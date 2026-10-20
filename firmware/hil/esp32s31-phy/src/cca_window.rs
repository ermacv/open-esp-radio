//! Diagnostic-only ROM counter-window arm, after the unchanged capture.
// SOURCE: pinned ESP32-S31 rev0 ROM ELF SHA-256
// a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87,
// phy_set_cca_cnt at 0x2f8256cc, 46 bytes. It replaces +0x58 bits 26:0
// with arg0 and, if arg1 is nonzero, sets bits 28:27. No stack or calls.
unsafe extern "C" {
    fn phy_set_cca_cnt(window: u32, start: u32);
}

// The baseline 0x10000 window left the busy result at zero. Test the next
// power-of-16 window on a separate boot: changing it also lets +0x5c prove
// that a fresh measurement ran. No undocumented tick-to-time conversion.
pub const WINDOW: u32 = 0x100000;

pub fn start() {
    // SAFETY: the sole P2 radio owner calls this with registered, powered,
    // clocked PHY and live Wi-Fi RX; no other PHY operation runs concurrently.
    // The ROM leaf has integer arguments only and accesses the known control
    // word; its address comes from the existing chip ROM linker definition.
    unsafe { phy_set_cca_cnt(WINDOW, 1) };
}
