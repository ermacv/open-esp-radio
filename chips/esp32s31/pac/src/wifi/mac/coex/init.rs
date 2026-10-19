//! Generated-PAC ownership for the complete cold COEX/PTI transaction.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

/// Live value-only projection of the cold MAC coexistence priorities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacCoexPrioritySnapshot {
    pub rx_active: u8,
    pub rx_ack: u8,
    pub wifi_default: u8,
}

impl WifiRadioRegisters {
    /// Read back the priorities that arbitrate ordinary Wi-Fi work and the
    /// immediate RX response transaction.
    pub fn mac_coex_priority_snapshot(&self) -> MacCoexPrioritySnapshot {
        let coex = &self.peripherals.wifi_mac.wifi_mac_coex_init;
        let rx = coex.rx_pti().read();
        MacCoexPrioritySnapshot {
            rx_active: rx.rx_active().bits(),
            rx_ack: rx.rx_ack().bits(),
            wifi_default: coex.default_control().read().wifi_default_pti().bits(),
        }
    }

    /// Apply all seventeen fresh-read RMW edges at the COEX tail of `hal_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0x12e..0x190`, and all complete `hal_coex.o`/`hal_mac_ctl.o`
    /// setter leaves reached by that tail.
    pub fn initialize_mac_coex(
        &mut self,
        rx_ack: u8,
        wifi_default: u8,
        tb: [u8; 7],
        beamforming: [u8; 3],
        multi_target: [u8; 2],
    ) {
        let coex = &self.peripherals.wifi_mac.wifi_mac_coex_init;

        coex.default_control()
            .modify(|_, w| w.bit_5_opaque().set_bit());
        coex.default_control()
            .modify(|_, w| w.default_pti_enable().set_bit());

        // Preserve the separate setter RMWs from complete hal_init.
        coex.rx_pti().modify(|_, w| w.rx_active().set(0));
        coex.rx_pti().modify(|_, w| w.rx_ack().set(rx_ack & 0x0f));
        coex.default_control()
            .modify(|_, w| w.wifi_default_pti().set(wifi_default & 0x0f));

        let tb_and_beamforming = coex.ofdma_tb_and_beamforming();
        // Complete hal_set_tb_pti uses argument order 0,1,2,3,5,6,4.
        tb_and_beamforming.modify(|_, w| w.tb_0().set(tb[0] & 0x0f));
        tb_and_beamforming.modify(|_, w| w.tb_1().set(tb[1] & 0x0f));
        tb_and_beamforming.modify(|_, w| w.tb_2().set(tb[2] & 0x0f));
        tb_and_beamforming.modify(|_, w| w.tb_3().set(tb[3] & 0x0f));
        tb_and_beamforming.modify(|_, w| w.tb_5().set(tb[5] & 0x0f));
        tb_and_beamforming.modify(|_, w| w.tb_6().set(tb[6] & 0x0f));
        tb_and_beamforming.modify(|_, w| w.tb_4().set(tb[4] & 0x0f));

        // Complete hal_set_beamf_pti spans the high nibble of the preceding
        // word and the low two nibbles of the following word.
        tb_and_beamforming.modify(|_, w| w.beamforming_0().set(beamforming[0] & 0x0f));
        let beamforming_control = coex.beamforming();
        beamforming_control.modify(|_, w| w.beamforming_1().set(beamforming[1] & 0x0f));
        beamforming_control.modify(|_, w| w.beamforming_2().set(beamforming[2] & 0x0f));

        // The complete multi-target setter writes argument one first, then
        // writes the unmasked eight-bit argument zero into a ten-bit field.
        beamforming_control.modify(|_, w| w.multi_target_1().set(multi_target[1] & 0x0f));
        beamforming_control.modify(|_, w| w.multi_target_0().set(u16::from(multi_target[0])));
    }
}
