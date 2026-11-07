//! The Espressif station's Block Ack policy: the TX agreements it
//! negotiates once connected, its Dialog Token sequence and how long a
//! receive reorder window waits for a missing MPDU.

/// TX Block Ack TIDs the vendor station negotiates when its connection
/// completes, in order.
///
/// SOURCE(esp32s31): complete `libnet80211[wl_cnx.o]::cnx_auth_done`
/// invokes `ieee80211_ampdu_request` for TIDs 0, 7 and 5 in this order.
pub const STA_TX_BLOCK_ACK_TIDS: [u8; 3] = [0, 7, 5];

/// The first Dialog Token of the vendor's sequence.
pub const FIRST_DIALOG_TOKEN: u8 = 1;

/// The Dialog Token that follows `current` in the vendor's sequence: one
/// archive-static token shared by every agreement, modulo 63.
///
/// SOURCE(esp32s31): complete `libnet80211[ieee80211_ht.o]::ieee80211_ampdu_request`
/// increments one archive-static token modulo 63.
pub const fn next_dialog_token(current: u8) -> u8 {
    if current >= 62 {
        0
    } else {
        current.saturating_add(1)
    }
}

/// How long a receive reorder window holds a buffered run behind a missing
/// MPDU before it releases the run past the gap, from the first MPDU it
/// retains.
///
/// SOURCE(esp32s31): complete `libnet80211[ieee80211_ht.o]::ieee80211_ampdu_reorder`
/// starts its reorder age timer with 0x493e0 exactly when the first frame is
/// retained; the timer is armed through the microsecond OSI timer-arm slot,
/// a 300,000 us edge.
pub const RX_REORDER_GAP_TIMEOUT_MICROS: u64 = 300_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_token_wraps_modulo_sixty_three() {
        assert_eq!(next_dialog_token(FIRST_DIALOG_TOKEN), 2);
        assert_eq!(next_dialog_token(61), 62);
        assert_eq!(next_dialog_token(62), 0);
        assert_eq!(next_dialog_token(0), 1);
    }
}
