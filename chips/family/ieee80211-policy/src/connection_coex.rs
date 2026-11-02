//! The per-frame coexistence requests of a reconnecting station.
//!
//! After a station loses its association while another radio shares the air,
//! the vendor turns on its coexistence reconnect policy until the next
//! association starts power management. Under that policy each Probe Request
//! requests event 45, and each Authentication, (Re)Association Request and
//! EAPOL frame requests event 46 and carries that event's priority with a
//! priority count of 4000 instead of its ordinary one.
//!
//! SOURCE(esp32s31): complete pinned `libpp[pp.o]::pp_coex_tx_request`, which
//! `libpp[pp.o]::ppProcessTxQ` calls before `libpp[lmac.o]::lmacTxFrame`; `libpp[pm_coex.o]::pm_coex_reconnect_policy` and `pm_coex_set_reconnect_policy`;
//! `libpp[hal_mac.o]::mac_tx_set_pti`, which publishes the unsigned
//! minimum of the frame and slice priorities as the scheduler priority.

use core::future::Future;
use oer_ieee80211_contracts_port::ConnectionFrame;

/// The priorities of a connection frame under the reconnect policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectFramePriority {
    /// Event 46's priority, as the frame's packet priority.
    pub packet: u8,
    /// The lesser of the packet priority and the slice event's priority.
    pub scheduler: u8,
}

impl ReconnectFramePriority {
    /// The priority count the vendor stores beside event 46's priority.
    pub const COUNT: u16 = 4_000;
}

/// The coexistence request of one connection frame about to be published.
pub trait ConnectionFrameCoex {
    /// Request the air for `frame` when the reconnect policy is on, and
    /// return the priorities it then carries; `None` keeps its ordinary
    /// priorities. A Probe Request only requests the air.
    fn connection_frame(
        &mut self,
        frame: ConnectionFrame,
    ) -> impl Future<Output = Option<ReconnectFramePriority>> + '_;
}
