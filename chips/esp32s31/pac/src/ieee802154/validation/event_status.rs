//! Closed IEEE 802.15.4 `EVENT_STATUS` validation transaction.
//!
//! This module is compiled only into validation images. Production code uses
//! the generated affine W1C snapshot. The named timer-zero and timer-one
//! accessors remain isolated here solely to preserve the historical selective
//! clearing discriminator; neither accepts a caller-provided image.

#[inline]
fn order_device_accesses() {
    crate::svd::device_access::fence();
}

/// Enable only the two validation timer events.
#[inline]
pub fn enable_timer_events(registers: &crate::svd::Ieee802154Mac) {
    registers.event_enable().modify(|_, writer| {
        writer.tx_done().clear_bit();
        writer.rx_done().clear_bit();
        writer.ack_tx_done().clear_bit();
        writer.ack_rx_done().clear_bit();
        writer.rx_abort().clear_bit();
        writer.tx_abort().clear_bit();
        writer.ed_done().clear_bit();
        writer.bit_7_opaque().clear_bit();
        writer.timer0_overflow().set_bit();
        writer.timer1_overflow().set_bit();
        writer.clock_count_match().clear_bit();
        writer.tx_sfd_done().clear_bit();
        writer.rx_sfd_done().clear_bit();
        writer.bit_13_opaque().clear_bit()
    });
    order_device_accesses();
}

/// Replace only the event-enable field with zero during cleanup.
#[inline]
pub fn disable_all_events(registers: &crate::svd::Ieee802154Mac) {
    registers.event_enable().modify(|_, writer| {
        writer.tx_done().clear_bit();
        writer.rx_done().clear_bit();
        writer.ack_tx_done().clear_bit();
        writer.ack_rx_done().clear_bit();
        writer.rx_abort().clear_bit();
        writer.tx_abort().clear_bit();
        writer.ed_done().clear_bit();
        writer.bit_7_opaque().clear_bit();
        writer.timer0_overflow().clear_bit();
        writer.timer1_overflow().clear_bit();
        writer.clock_count_match().clear_bit();
        writer.tx_sfd_done().clear_bit();
        writer.rx_sfd_done().clear_bit();
        writer.bit_13_opaque().clear_bit()
    });
    order_device_accesses();
}

/// Return one semantic `EVENT_STATUS` sample through generated field readers.
#[inline]
pub fn event_status_events(
    registers: &crate::svd::Ieee802154Mac,
) -> crate::ieee802154::ownership::Ieee802154EventReadback {
    order_device_accesses();
    crate::ieee802154::ownership::Ieee802154EventReadback::from_event_status(
        &registers.event_status().read(),
    )
}

/// Return one complete timer-zero counter sample.
#[inline]
pub fn timer0_value(registers: &crate::svd::Ieee802154Mac) -> u32 {
    order_device_accesses();
    registers.timer0_value().read().value().bits()
}

/// Return one complete timer-one counter sample.
#[inline]
pub fn timer1_value(registers: &crate::svd::Ieee802154Mac) -> u32 {
    order_device_accesses();
    registers.timer1_value().read().value().bits()
}

/// Program both validation timers with the same nonzero threshold.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn set_timer_thresholds(registers: &crate::svd::Ieee802154Mac, threshold: u32) {
    // SAFETY: both generated registers are complete 32-bit write-only words.
    // The validation HAL admits only a nonzero field-sized threshold.
    unsafe {
        registers
            .timer0_threshold()
            .write_with_zero(|writer| writer.threshold().bits(threshold));
        registers
            .timer1_threshold()
            .write_with_zero(|writer| writer.threshold().bits(threshold));
    }
    order_device_accesses();
}

/// Start validation timer zero through its source-confirmed opcode.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn start_timer0(registers: &crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field admits this finite public-LL opcode and the
    // command is issued only by the closed validation transaction.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_start());
    }
    order_device_accesses();
}

/// Stop validation timer zero through its source-confirmed opcode.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn stop_timer0(registers: &crate::svd::Ieee802154Mac) {
    // SAFETY: see [`start_timer0`]; this is the paired finite stop opcode.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_stop());
    }
    order_device_accesses();
}

/// Start validation timer one through its source-confirmed opcode.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn start_timer1(registers: &crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field admits this finite public-LL opcode and the
    // command is issued only by the closed validation transaction.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer1_start());
    }
    order_device_accesses();
}

/// Stop validation timer one through its source-confirmed opcode.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn stop_timer1(registers: &crate::svd::Ieee802154Mac) {
    // SAFETY: see [`start_timer1`]; this is the paired finite stop opcode.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer1_stop());
    }
    order_device_accesses();
}

/// Select only timer zero through its generated W1C field variant.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn write_timer0_event(registers: &crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field accessor selects only TIMER0 in this
    // reset-isolated validation transaction.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.timer0_overflow().bit(true));
    }
    order_device_accesses();
}

/// Select only timer one through its generated W1C field variant.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn write_timer1_event(registers: &crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field accessor selects only TIMER1 in this
    // reset-isolated validation transaction.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.timer1_overflow().bit(true));
    }
    order_device_accesses();
}
