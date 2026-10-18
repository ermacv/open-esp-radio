//! ABI and completion-edge delivery for shipping I2C transactions.
use oer_esp32s31_phy::calibration::cold::{
    PhyColdI2cAction, PhyColdI2cOutcome, PhyColdI2cRequest, PhyColdI2cTransaction,
};

oer_probe_macros::probe! {
    /// Invoke two captured void entries in caller-selected order with zero ABI words.
    /// No child result or algorithm is synthesized; both bodies execute normally.
    ///
    /// # Safety
    /// Both addresses must identify executable void functions accepting zero input
    /// words. The caller supplies a valid stack and any memory those functions need.
    #[unsafe(naked)]
    pub unsafe fn open_phy_trace_two_void_entries(first: u32, second: u32) {
        core::arch::naked_asm!(
            "addi sp, sp, -16",
            "sw ra, 12(sp)",
            "sw a1, 8(sp)",
            "mv t0, a0",
            "li a0, 0",
            "li a1, 0",
            "li a2, 0",
            "li a3, 0",
            "li a4, 0",
            "li a5, 0",
            "li a6, 0",
            "li a7, 0",
            "jalr t0",
            "lw t0, 8(sp)",
            "lw ra, 12(sp)",
            "addi sp, sp, 16",
            "li a0, 0",
            "li a1, 0",
            "li a2, 0",
            "li a3, 0",
            "li a4, 0",
            "li a5, 0",
            "li a6, 0",
            "li a7, 0",
            "jr t0",
        );
    }
}

oer_probe_macros::probe! {
    /// Supply a finite sequence of completion edges to the production transaction.
    /// Profiles 0..3 select ADC-rate byte read/write/field read/write; 4..7 select
    /// the TX-capacitor high field. Encodings and masked updates remain production code.
    /// Every start or delivered edge consumes one harness action. Return 0x10000
    /// denotes start failure, 0x10001 an exhausted action schedule, 0x10002 an invalid
    /// transition and 0x10003 an unknown profile. This scheduling is not hardware time.
    pub fn open_phy_trace_i2c_transfer(profile: u32, value: u32, maximum_actions: u32) -> u32 {
        use oer_esp32s31_hal::phy::i2c::analog_registers::{ADC_RATE_CONFIGURATION, TX_CAPACITOR_HIGH};
        let field = match profile {
            0..=3 => ADC_RATE_CONFIGURATION,
            4..=7 => TX_CAPACITOR_HIGH,
            _ => return 0x10003,
        };
        let request = match profile % 4 {
            0 => PhyColdI2cRequest::read_byte(field.address()),
            1 => PhyColdI2cRequest::write_byte(field.address(), value as u8),
            2 => PhyColdI2cRequest::read_field(field),
            _ => PhyColdI2cRequest::write_field(field, value as u8),
        };
        super::with_phy(|registers| {
            let mut transaction = PhyColdI2cTransaction::new(request);
            for _ in 0..maximum_actions {
                match transaction.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        if transaction.start_target(&mut crate::shared_phy(registers)).is_err() {
                            return 0x10000;
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        if transaction.observe_target_edge(&crate::shared_phy(registers)).is_err() {
                            return 0x10002;
                        }
                    }
                    PhyColdI2cAction::Complete(_) => break,
                }
            }
            match transaction.action() {
                PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read { value, .. }) => u32::from(value),
                PhyColdI2cAction::Complete(PhyColdI2cOutcome::Written { .. }) => 0,
                _ => 0x10001,
            }
        })
    }
}

oer_probe_macros::probe! {
    /// Lower the shipping host-selection result to the captured integer ABI.
    pub fn open_phy_trace_i2c_host(block: u32) -> u32 {
        super::with_phy(|registers| {
            oer_esp32s31_phy::validation::configure_and_select_phy_i2c_host(&mut crate::shared_phy(registers), block as u8)
        })
    }
}

oer_probe_macros::probe! {
    /// Execute the complete production reset helper, including its bounded poll policy.
    pub fn open_phy_trace_i2c_reset() -> u32 {
        super::with_phy(
            |registers| match oer_esp32s31_phy::validation::reset_i2c_master(&mut crate::shared_phy(registers)) {
                Ok(()) => 0,
                Err(oer_esp32s31_phy::PhyTargetPortError::HardwareEdgeTimedOut) => 0x10001,
                Err(_) => 0x10002,
            },
        )
    }
}
