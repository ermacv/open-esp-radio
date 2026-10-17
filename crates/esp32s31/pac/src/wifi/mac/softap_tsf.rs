//! Safe generated-PAC ownership for recovered SoftAP TSF lifecycle edges.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

impl WifiRadioRegisters {
    /// Reset the SoftAP TSF value and start its hardware domain.
    ///
    /// SOURCE: the `arg0 == 0` path of complete
    /// `libpp.a[hal_mac.o]::hal_mac_tsf_reset`. The vendor code first clears
    /// both high control bits, clears the shared TSF load words, asserts the
    /// SoftAP load request and finally sets both high control bits through a
    /// fresh read-modify-write.
    pub fn reset_and_start_softap_tsf(&mut self) {
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_aux_tsf_control
            .softap_control();
        control.modify(|_, writer| writer.tsf_enable().clear_bit().bit_30_opaque().clear_bit());

        let load = &self.peripherals.wifi_mac.wifi_mac_tsf_load;
        crate::generated::station_tsf_value_low(load, crate::generated::StationTsfLowWord::new(0));
        crate::generated::station_tsf_value_high(
            load,
            crate::generated::StationTsfHighWord::new(0),
        );
        load.control()
            .modify(|_, writer| writer.load_softap_tsf().set_bit());

        control.modify(|_, writer| writer.tsf_enable().set_bit().bit_30_opaque().set_bit());
    }

    /// Stop the SoftAP TSF domain using the complete vendor leaf transaction.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::hal_disable_softap_tsf`
    /// clears bits 31 and 30 of `0x2010_d860` with one read-modify-write.
    /// Bit 31 is the TSF enable. Bit 30 is deliberately left unnamed by the
    /// recovered SVD because no complete enable leaf proves its semantics.
    pub fn stop_softap_tsf(&mut self) {
        self.peripherals
            .wifi_mac
            .wifi_mac_aux_tsf_control
            .softap_control()
            .modify(|_, writer| writer.tsf_enable().clear_bit().bit_30_opaque().clear_bit());
    }
}
