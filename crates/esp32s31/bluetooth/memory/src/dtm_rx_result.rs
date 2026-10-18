//! Typed result from one returned DTM RX packet slot.

#![forbid(unsafe_code)]

/// Signed RSSI sample produced by one accepted DTM RX packet.
///
/// The vendor getter returns this value with a signed byte load. The unit and
/// calibration remain controller-owned; this type does not claim dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmRxRssi(i8);

impl DtmRxRssi {
    /// Preserve one signed controller value without assigning a physical unit.
    pub const fn from_controller_value(value: i8) -> Self {
        Self(value)
    }

    /// Return the signed controller value without assigning a physical unit.
    pub const fn controller_value(self) -> i8 {
        self.0
    }
}

/// Validated result word consumed by the reviewed ESP32-S31 DTM RX callback.
///
/// Complete pinned body `libble_app.a[dtm_4.o]::sym_dtm_ikQ7xyHA5qBHWlaqAjZu`
/// reads this word at returned packet-buffer offset `+0x0c`. It accepts the
/// word only when its low 24 bits are zero and copies the high byte from
/// offset `+0x0f` into DTM state. This controller-memory projection assigns no
/// meaning to the individual low result bits. `esp_ble_get_dtm_rx_rssi`
/// reaches `sym_dtm_JMPstxdIRcEVY1YJeXkA` through two tail calls, and that
/// complete body returns the same DTM state byte with a signed load. This
/// closes the high-byte role as RSSI while leaving the low-bit failure
/// meanings and physical unit unresolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmRxResultProjection(DtmRxRssi);

/// Why a positional DTM RX result word is not counted by the reviewed path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRxResultProjectionError {
    /// At least one of the low 24 bits is nonzero.
    NonzeroLowTwentyFourBits,
}

impl DtmRxResultProjection {
    /// Apply the exact validation performed at returned-buffer offset `+0x0c`.
    pub const fn from_word(word: u32) -> Result<Self, DtmRxResultProjectionError> {
        if word & 0x00ff_ffff != 0 {
            return Err(DtmRxResultProjectionError::NonzeroLowTwentyFourBits);
        }
        Ok(Self(DtmRxRssi((word >> 24) as u8 as i8)))
    }

    /// Return the signed RSSI copied from returned-buffer offset `+0x0f`.
    pub const fn rssi(self) -> DtmRxRssi {
        self.0
    }
}
