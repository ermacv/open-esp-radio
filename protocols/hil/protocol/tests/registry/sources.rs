//! Compile-time inputs for the hermetic registry checks.
pub const FRAMEWORK: &[(&str, &str)] = &[
    ("lib.rs", include_str!("../../src/lib.rs")),
    ("key.rs", include_str!("../../src/key.rs")),
    ("framing.rs", include_str!("../../src/framing.rs")),
    ("io.rs", include_str!("../../src/io.rs")),
    ("envelope.rs", include_str!("../../src/envelope.rs")),
];
pub const MODULES: &[(&str, &[&str])] = &[
    (
        "base",
        &[
            include_str!("../../src/base/boot.rs"),
            include_str!("../../src/base/image_keys.rs"),
            include_str!("../../src/base/mod.rs"),
        ],
    ),
    (
        "system",
        &[
            include_str!("../../src/system/mod.rs"),
            include_str!("../../src/system/payload.rs"),
        ],
    ),
    (
        "network",
        &[
            include_str!("../../src/network/mod.rs"),
            include_str!("../../src/network/session.rs"),
            include_str!("../../src/network/state.rs"),
            include_str!("../../src/network/stream_pattern.rs"),
            include_str!("../../src/network/traffic.rs"),
            include_str!("../../src/network/udp_probe.rs"),
        ],
    ),
    (
        "bluetooth",
        &[
            include_str!("../../src/bluetooth/gatt.rs"),
            include_str!("../../src/bluetooth/mod.rs"),
            include_str!("../../src/bluetooth/payload.rs"),
            include_str!("../../src/bluetooth/secure_gatt.rs"),
        ],
    ),
    (
        "telemetry",
        &[
            include_str!("../../src/telemetry/mod.rs"),
            include_str!("../../src/telemetry/profile.rs"),
        ],
    ),
    (
        "phy",
        &[
            include_str!("../../src/phy/capture.rs"),
            include_str!("../../src/phy/hold.rs"),
            include_str!("../../src/phy/mod.rs"),
        ],
    ),
    (
        "lower-mac",
        &[
            include_str!("../../src/lower_mac/mod.rs"),
            include_str!("../../src/lower_mac/payload.rs"),
        ],
    ),
];
