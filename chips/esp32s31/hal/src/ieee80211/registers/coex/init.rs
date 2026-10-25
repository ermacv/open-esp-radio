//! The complete cold COEX/PTI transaction.

use oer_esp32s31_pac::MacPti;
use oer_mmio::Bits;

use crate::ieee80211::registers::{MacPlacement, WifiRadioRegisters, bits, byte};

/// Live value-only projection of the cold MAC coexistence priorities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacCoexPrioritySnapshot {
    pub rx_active: u8,
    pub rx_ack: u8,
    pub wifi_default: u8,
}

/// The four-bit field value of a PTI.
pub(crate) const fn pti(value: MacPti) -> Bits<4> {
    bits(value.get())
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Read back the priorities that arbitrate ordinary Wi-Fi work and the
    /// immediate RX response transaction.
    pub fn mac_coex_priority_snapshot(&self) -> MacCoexPrioritySnapshot {
        let coex = &self.mac.wifi_mac_coex_init;
        let rx = coex.read_rx_pti();
        MacCoexPrioritySnapshot {
            rx_active: byte(rx.rx_active()),
            rx_ack: byte(rx.rx_ack()),
            wifi_default: byte(coex.read_default_control().wifi_default_pti()),
        }
    }

    /// Apply all seventeen fresh-read RMW edges at the COEX tail of `hal_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0x12e..0x190`, and all complete `hal_coex.o`/`hal_mac_ctl.o`
    /// setter leaves reached by that tail.
    pub fn initialize_mac_coex(
        &mut self,
        rx_ack: MacPti,
        wifi_default: MacPti,
        tb: [MacPti; 7],
        beamforming: [MacPti; 3],
        multi_target_0: u8,
        multi_target_1: MacPti,
    ) {
        let coex = &mut self.mac.wifi_mac_coex_init;

        coex.modify_default_control(|w| {
            w.set_bit_5_opaque(true);
        });
        coex.modify_default_control(|w| {
            w.set_default_pti_enable(true);
        });

        // Preserve the separate setter RMWs from complete hal_init.
        coex.modify_rx_pti(|w| {
            w.set_rx_active(Bits::constant::<0>());
        });
        coex.modify_rx_pti(|w| {
            w.set_rx_ack(pti(rx_ack));
        });
        coex.modify_default_control(|w| {
            w.set_wifi_default_pti(pti(wifi_default));
        });

        // Complete hal_set_tb_pti uses argument order 0,1,2,3,5,6,4.
        let [tb0, tb1, tb2, tb3, tb4, tb5, tb6] = tb;
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_0(pti(tb0));
        });
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_1(pti(tb1));
        });
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_2(pti(tb2));
        });
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_3(pti(tb3));
        });
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_5(pti(tb5));
        });
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_6(pti(tb6));
        });
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_tb_4(pti(tb4));
        });

        // Complete hal_set_beamf_pti spans the high nibble of the preceding
        // word and the low two nibbles of the following word.
        let [beamforming_0, beamforming_1, beamforming_2] = beamforming;
        coex.modify_ofdma_tb_and_beamforming(|w| {
            w.set_beamforming_0(pti(beamforming_0));
        });
        coex.modify_beamforming(|w| {
            w.set_beamforming_1(pti(beamforming_1));
        });
        coex.modify_beamforming(|w| {
            w.set_beamforming_2(pti(beamforming_2));
        });

        // The complete multi-target setter writes argument one first, then
        // writes the unmasked eight-bit argument zero into a ten-bit field.
        coex.modify_beamforming(|w| {
            w.set_multi_target_1(pti(multi_target_1));
        });
        coex.modify_beamforming(|w| {
            w.set_multi_target_0(bits(u32::from(multi_target_0)));
        });
    }
}
