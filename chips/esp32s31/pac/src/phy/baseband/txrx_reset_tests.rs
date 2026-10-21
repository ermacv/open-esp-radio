//! Host-only execution of the generated frontend reset against anonymous memory.

#[test]
#[allow(
    unsafe_code,
    clippy::disallowed_methods,
    reason = "run the actual generated fixed-address transactions against private anonymous host memory"
)]
fn frontend_reset_preserves_poisoned_lanes_across_both_rom_edges() {
    struct Mapping(*mut libc::c_void);
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: this guard uniquely owns the successful page mapping.
            unsafe { libc::munmap(self.0, 0x1000) };
        }
    }

    // SAFETY: anonymous private memory, with no descriptor. NOREPLACE fails
    // rather than replacing an existing host mapping at this hardware address.
    let page = unsafe {
        libc::mmap(
            0x2010_0000 as *mut libc::c_void,
            0x1000,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_FIXED_NOREPLACE,
            -1,
            0,
        )
    };
    assert_ne!(page, libc::MAP_FAILED, "cannot map the private test page");
    let _mapping = Mapping(page);
    // SAFETY: this test owns the mapped page and the only peripheral instance;
    // no hardware is accessed and this instance never escapes into a HAL.
    let registers = unsafe { crate::svd::PhyFedataRecovered::steal() };
    let word = 0x2010_0440 as *mut u32;
    for (initial, cleared, asserted) in [
        (0xffff_ffff, 0xf9ff_ffff, 0xffff_ffff),
        (0x01ff_f000, 0x01ff_f000, 0x07ff_f000),
        (0x0600_0000, 0x0000_0000, 0x0600_0000),
    ] {
        // SAFETY: aligned word inside the private, live read/write mapping.
        unsafe { core::ptr::write_volatile(word, initial) };
        crate::generated::clear_phy_frontend_txrx_reset_state(&registers);
        assert_eq!(registers.word_0040_opaque().read().bits(), cleared);
        crate::generated::assert_phy_frontend_txrx_reset_state(&registers);
        assert_eq!(registers.word_0040_opaque().read().bits(), asserted);
    }
    // A hardware update between edges must survive the second fresh read.
    crate::generated::clear_phy_frontend_txrx_reset_state(&registers);
    // SAFETY: the same private mapped word, exclusively owned by this test.
    unsafe { core::ptr::write_volatile(word, 0x9155_aa55) };
    crate::generated::assert_phy_frontend_txrx_reset_state(&registers);
    assert_eq!(registers.word_0040_opaque().read().bits(), 0x9755_aa55);
}
