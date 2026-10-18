#![no_std]
#![forbid(unsafe_code)]

//! Radio APIs, portable protocols and explicitly selected hardware compositions.
//!
//! The default portable Wi-Fi protocols build on the host without a chip or
//! executor. Firmware selects its chip and compositions through Cargo features.

pub use {memory, network};

#[cfg(feature = "wifi")]
pub mod ieee80211 {
    pub use {ap, datapath, mac, softmac, sta};

    pub mod security {
        pub use rsn;
    }

    /// Executor-independent drivers of the security state machines.
    pub mod services {
        pub use rsn_service as rsn;
    }
}

#[cfg(feature = "bluetooth")]
pub mod bluetooth {
    pub use hci;

    pub mod le {
        pub use le_ll as ll;
    }
}

#[cfg(feature = "i")]
pub use ieee802154;

#[cfg(feature = "esp32s31")]
pub mod chips {
    pub mod esp32s31 {
        pub use chip_hal as hal;

        pub mod driver {
            /// Bluetooth hardware engine.
            #[cfg(feature = "esp32s31-bluetooth")]
            pub use chip_bluetooth as bluetooth;

            /// IEEE 802.15.4 MAC engine.
            #[cfg(feature = "esp32s31-i")]
            pub use chip_ieee802154 as ieee802154;
        }
    }
}
