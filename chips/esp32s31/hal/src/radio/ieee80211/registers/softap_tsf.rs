//! The recovered SoftAP TSF lifecycle edges.

use super::{MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Reset the SoftAP TSF value and start its hardware domain.
    ///
    /// SOURCE: the `arg0 == 0` path of complete
    /// `libpp.a[hal_mac.o]::hal_mac_tsf_reset`. The vendor code first clears
    /// both high control bits, clears the shared TSF load words, asserts the
    /// SoftAP load request and finally sets both high control bits through a
    /// fresh read-modify-write.
    pub fn reset_and_start_softap_tsf(&mut self) {
        let control = &mut self.mac.wifi_mac_aux_tsf_control;
        control.modify_softap_control(|w| {
            w.set_tsf_enable(false).set_bit_30_opaque(false);
        });

        let load = &mut self.mac.wifi_mac_tsf_load;
        load.write_value_low(|w| {
            w.set_value(0);
        });
        load.write_value_high(|w| {
            w.set_value(0);
        });
        load.modify_tsf_load_control(|w| {
            w.set_load_softap_tsf(true);
        });

        self.mac
            .wifi_mac_aux_tsf_control
            .modify_softap_control(|w| {
                w.set_tsf_enable(true).set_bit_30_opaque(true);
            });
    }

    /// Stop the SoftAP TSF domain using the complete vendor leaf transaction.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::hal_disable_softap_tsf`
    /// clears bits 31 and 30 of `0x2010_d860` with one read-modify-write.
    /// Bit 31 is the TSF enable. Bit 30 is deliberately left unnamed by the
    /// recovered model because no complete enable leaf proves its semantics.
    pub fn stop_softap_tsf(&mut self) {
        self.mac
            .wifi_mac_aux_tsf_control
            .modify_softap_control(|w| {
                w.set_tsf_enable(false).set_bit_30_opaque(false);
            });
    }
}

#[cfg(test)]
mod tests;
