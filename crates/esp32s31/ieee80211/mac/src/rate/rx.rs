//! Stateless receive-protocol classification recovered from ESP32-S31 PP.

/// Numeric rate-control context selected by the pinned
/// `libpp.a[pp.o]::ppRxProtoProc` body.
///
/// The values are part of the private PP ABI. Their concrete vendor enum
/// names are not available, so this type deliberately exposes only the
/// recovered numeric identity. Route 3 means that no rate-control context is
/// looked up or updated and is therefore represented by `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxRateControlRoute(u8);

impl RxRateControlRoute {
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Reproduce the complete route selection made from RX-control byte 3.
///
/// Bits outside `0x10 | 0x20 | 0x40` do not participate. This function owns
/// no state and performs no polling, waiting, allocation, or indirect call.
pub const fn rate_control_route(rx_flags: u8) -> Option<RxRateControlRoute> {
    if rx_flags & 0x10 != 0 {
        if rx_flags & 0x20 != 0 {
            None
        } else {
            Some(RxRateControlRoute(0))
        }
    } else if rx_flags & 0x20 != 0 {
        Some(RxRateControlRoute(1))
    } else if rx_flags & 0x40 != 0 {
        Some(RxRateControlRoute(2))
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxRateSampleUpdate {
    pub latest: i8,
    pub smoothed: i8,
}

/// Reproduce the two-byte receive-signal filter from `rcUpdateRxDone`.
///
/// The first guard is rate-context flag `0x80`; the second is bit zero at
/// context offset `0x1b`. The hardware sample and calibration byte add with
/// eight-bit wrapping before interpretation as a signed RSSI value.
pub const fn update_rx_rate_sample(
    context_flags: u16,
    state_flags: u8,
    calibration: u8,
    raw_sample: u8,
    previous_latest: i8,
    previous_smoothed: i8,
) -> Option<RxRateSampleUpdate> {
    if context_flags & 0x80 != 0 || state_flags & 1 != 0 {
        return None;
    }

    let sample = raw_sample.wrapping_add(calibration) as i8;
    let intermediate = if previous_latest == 127 {
        sample
    } else {
        ((previous_latest as i16 + sample as i16) >> 1) as i8
    };
    let smoothed = if previous_smoothed == 127 {
        intermediate
    } else {
        ((previous_smoothed as i16 * 3 + intermediate as i16) / 4) as i8
    };
    Some(RxRateSampleUpdate {
        latest: sample,
        smoothed,
    })
}

#[cfg(test)]
mod tests;
