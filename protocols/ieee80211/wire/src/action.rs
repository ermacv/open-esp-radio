//! The codes of Action frames: their categories, and each category's
//! actions.

use oer_portable_wire::octet_code;

octet_code! { "oer_protocols_ieee80211_wire::action";
    /// An Action frame's category: any octet, some of them named.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.11 (Action field).
    ActionCategory {
        /// Spectrum Management.
        SPECTRUM_MANAGEMENT = 0,
        /// Block Ack.
        BLOCK_ACK = 3,
        /// Public.
        PUBLIC = 4,
        /// SA Query.
        SA_QUERY = 8,
        /// S1G, which carries TWT.
        ///
        /// SOURCE: Linux v6.1 `include/linux/ieee80211.h`.
        S1G = 22,
        /// Vendor-specific.
        VENDOR_SPECIFIC = 127,
    }
}

impl ActionCategory {
    /// Whether an Action frame of this category is robust.
    ///
    /// SOURCE(esp32s31): complete pinned
    /// `libnet80211[ieee80211_sta.o]::ieee80211_is_action_category_robust`.
    pub const fn is_robust(self) -> bool {
        // Not robust: Public, HT (7), Unprotected WNM (11), Self-protected
        // (15), 20, 21, S1G and Vendor-specific.
        !matches!(self.0, 4 | 7 | 11 | 15 | 20 | 21 | 22 | 127)
    }
}

octet_code! { "oer_protocols_ieee80211_wire::action";
    /// An action of the Spectrum Management category.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.6.2 (Spectrum Management Action frame
    /// details).
    SpectrumManagementAction {
        /// Channel Switch Announcement.
        CHANNEL_SWITCH_ANNOUNCEMENT = 4,
    }
}

octet_code! { "oer_protocols_ieee80211_wire::action";
    /// An action of the Block Ack category.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.6.5 (Block Ack Action frame details).
    BlockAckActionCode {
        /// ADDBA Request.
        ADDBA_REQUEST = 0,
        /// ADDBA Response.
        ADDBA_RESPONSE = 1,
        /// DELBA.
        DELBA = 2,
    }
}

octet_code! { "oer_protocols_ieee80211_wire::action";
    /// An action of the Public category.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.6.7.1 (Public Action frames).
    PublicAction {
        /// Extended Channel Switch Announcement.
        EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT = 4,
        /// FTM Request.
        FTM_REQUEST = 32,
        /// FTM.
        FTM = 33,
    }
}

octet_code! { "oer_protocols_ieee80211_wire::action";
    /// An action of the SA Query category.
    ///
    /// SOURCE: IEEE Std 802.11-2016, 9.6.10 (SA Query).
    SaQueryAction {
        /// SA Query Request.
        REQUEST = 0,
        /// SA Query Response.
        RESPONSE = 1,
    }
}

octet_code! { "oer_protocols_ieee80211_wire::action";
    /// An action of the S1G category.
    ///
    /// SOURCE: Linux v6.1 `include/linux/ieee80211.h`.
    S1gAction {
        /// TWT Setup.
        TWT_SETUP = 6,
        /// TWT Teardown.
        TWT_TEARDOWN = 7,
    }
}
