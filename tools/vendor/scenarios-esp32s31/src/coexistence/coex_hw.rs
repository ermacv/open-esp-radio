//! Coexistence hardware and core leaves of the pinned `libcoexist.a`,
//! compared with the compiled production coexistence probes on the
//! `wifi-mac` leaf machinery.
use crate::harness::Result;
use crate::mac::{
    Domain, Leaf, Objects, Suite, Vendor, in_archive, leaf, objects, released,
    released_when_leased, stated, vendor_reads,
};
use oer_analysis_blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, RegionLifetime,
};

/// Hardware timers the production bank admits (`CoexTimerIndex`).
const TIMERS: &[u32] = &[0, 1, 2, 3, 4];

/// Coexistence events production admits (`CoexEventId`: below
/// `COEX_EVENT_COUNT`, 49).
const EVENTS: &[u32] = &events::<49>(None);
/// The admitted events and the first event beyond them.
const EVENTS_AND_INVALID: &[u32] = &events::<50>(None);
/// The PHY grant-protect event: vendor `coex_core_timer_idx_get` maps it to
/// timer 5, which production reserves for the radio arbiter's
/// `oer_esp32s31_hal::coex::PhyGrantProtect` instead of the driver's policy
/// timers (reviewed decision with the PHY owner, 2026-09-27). The driver
/// leaves compare every other event; the grant-protect leaves compare the
/// event through its own owner.
const GRANT_PROTECT_EVENT: u32 = 48;
/// The driver's events and the first event beyond them.
const DRIVER_EVENTS_AND_INVALID: &[u32] = &events::<49>(Some(GRANT_PROTECT_EVENT));

/// Events `0..` in order, skipping `excluded`.
const fn events<const N: usize>(excluded: Option<u32>) -> [u32; N] {
    let mut values = [0; N];
    let (mut index, mut event) = (0, 0);
    while index < N {
        if !matches!(excluded, Some(e) if e == event) {
            values[index] = event;
            index += 1;
        }
        event += 1;
    }
    values
}

/// The coexistence timer clock word `coex_hw_timer_tick_get` reads twice per
/// conversion: the low nibble selects the one-hot clock source and bits
/// 15:4 hold the divider minus one.
const CLOCK_SELECTOR: u32 = 0x2010_f008;
/// One clock word per source (8, 4, 2, 1), with nonzero dividers where the
/// conversion uses them.
const CLOCK_SELECTIONS: &[u32] = &[0x0000_0008, 0x0000_0314, 0x0000_0272, 0x0000_0001];

/// Vendor `g_coa_funcs_p`, a ROM cell, points at the coexistence adapter
/// table; slot 0x30 answers whether the chip is real and slot 0x48 the
/// crystal frequency in MHz.
const ADAPTER_CELL: &str = "g_coa_funcs_p";
const ADAPTER_TABLE: u32 = 0x3fff_3000;
const ADAPTER_BYTES: usize = 0x50;
const REAL_CHIP_SLOT: usize = 0x30;
const XTAL_SLOT: usize = 0x48;
/// Unmapped addresses the adapter slots point at, answered by call models.
const REAL_CHIP_MODEL: u32 = 0x5000_0000;
const XTAL_MODEL: u32 = 0x5000_0010;
/// The ESP32-S31 crystal the adapter reports, which production's clock
/// conversion also uses.
const XTAL_MHZ: u32 = 40;

/// Timer client field `coex_hw_timer_set` receives, by production client
/// (0 Bluetooth, 1 Wi-Fi): the pinned `libcoexist.a[coexist_core.o]`
/// `.rodata.CSWTCH.27` values `coex_core_request` maps request kinds 0 and 1
/// through. The request leaves compare that mapping against the vendor's
/// own table.
const CLIENT_FIELDS: [u32; 2] = [2, 1];

/// Whether the adapter reports a real chip.
const REAL_CHIP: &[u32] = &[0, 1];

/// A call model answering `value` at the unmapped `address`.
fn model(id: &str, address: u32, value: u32) -> CallDeclaration {
    CallDeclaration {
        id: id.into(),
        applicability: "a coexistence adapter query the case answers".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 0,
        responses: vec![CallResponse {
            return_words: [Some(value), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// The adapter table, its ROM cell and the models of its two queries, with
/// the case's clock word.
fn adapter(vendor: &Vendor<'_>, real_chip: u32, selector: u32) -> Result<Objects> {
    let mut table = vec![0u8; ADAPTER_BYTES];
    table[REAL_CHIP_SLOT..REAL_CHIP_SLOT + 4].copy_from_slice(&REAL_CHIP_MODEL.to_le_bytes());
    table[XTAL_SLOT..XTAL_SLOT + 4].copy_from_slice(&XTAL_MODEL.to_le_bytes());
    Ok(Objects {
        vendor: vec![
            (ADAPTER_TABLE, table),
            (
                vendor.symbol(ADAPTER_CELL)?,
                ADAPTER_TABLE.to_le_bytes().to_vec(),
            ),
        ],
        calls: vec![
            model("coex-adapter-real-chip", REAL_CHIP_MODEL, real_chip),
            model("coex-adapter-xtal", XTAL_MODEL, XTAL_MHZ),
        ],
        registers: vec![(CLOCK_SELECTOR, selector)],
        ..Default::default()
    })
}

/// `coex_hw_timer_set`: the probe's client becomes the vendor's field.
fn timer_set_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [index, client, pti, latency, duration, real_chip, selector] = *words else {
        unreachable!("timer words: index, client, PTI, latency, duration, real chip, clock")
    };
    Ok(Objects {
        vendor_words: vec![
            index,
            CLIENT_FIELDS[client as usize],
            pti,
            latency,
            duration,
        ],
        ..adapter(vendor, real_chip, selector)?
    })
}

/// `coex_hw_timer_freq_set`: the arguments pass unchanged over the case's
/// clock word.
fn freq_set_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let [selector, divisor, clock] = *words else {
        unreachable!("clock words: selector, divisor, clock")
    };
    Ok(Objects {
        vendor_words: vec![selector, divisor],
        registers: vec![(CLOCK_SELECTOR, clock)],
        ..Default::default()
    })
}

/// `coex_core_request`: the vendor maps the request kind itself.
fn request_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [client, event, latency, duration, real_chip, selector] = *words else {
        unreachable!("request words: client, event, latency, duration, real chip, clock")
    };
    Ok(Objects {
        vendor_words: vec![client, event, latency, duration],
        ..adapter(vendor, real_chip, selector)?
    })
}

/// The grant-protect entries take no arguments.
fn grant_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [selector] = *words else {
        unreachable!("grant words: clock")
    };
    adapter(vendor, 1, selector)
}

/// Production programs the grant-protect timer with zero latency and
/// duration, whose tick images need no clock; the vendor still converts them.
const GRANT_CLOCK_READ: &[(u32, &str)] = &[(
    CLOCK_SELECTOR,
    "the vendor converts the zero grant-protect latency and duration through the clock \
     word; production writes their zero tick images without sampling the clock",
)];

/// Coexistence priorities production admits (`CoexPti`: four bits); the
/// vendor field is five bits wide, but no production priority exceeds 15.
const PTIS: &[u32] = &events::<16>(None);
/// IEEE 802.15.4 coexistence levels (`ieee802154_coex_event_t`): the vendor
/// asserts on any other value.
const IEEE802154_LEVELS: &[u32] = &[1, 2, 3, 4];

/// A nullable output object of `length` bytes.
const fn output(length: u32) -> Domain {
    Domain::Output {
        offset: 0,
        length,
        nullable: true,
    }
}

/// External coexistence work modes production admits: leader (0) and
/// follower (2). The vendor's mode 1 and modes from 3 are outside
/// production, which rejects them without a write.
const EXTERNAL_MODES: &[u32] = &[0, 2];
const FOLLOWER_MODE: u32 = 2;
/// External coexistence wire types `esp_coex_external_get_wire_type`
/// reports.
const EXTERNAL_WIRES: &[u32] = &[0, 1, 2, 3];
/// Priority nibbles of `hal_set_extern_pti`: the bounds and both sides of
/// the top bit, so the three-priority product stays within one request's
/// case capacity over every mode, wire and register fill.
const EXTERNAL_PTIS: &[u32] = &[0, 7, 8, 15];
/// Grant delays in microseconds (`(delay << 4) & 0xf0`) and validity levels
/// of `ic_set_extern_coex_params`.
const EXTERNAL_DELAYS: &[u32] = &[0, 5, 15];
const EXTERNAL_VALIDITY: &[u32] = &[0, 1];
/// The captured wire-type query, answered with the case's wire.
const WIRE_QUERY: &str = "esp_coex_external_get_wire_type";

/// A model of the captured wire-type query answering `wire`.
fn wire_model(vendor: &Vendor<'_>, wire: u32) -> Result<CallDeclaration> {
    Ok(CallDeclaration {
        binding: CallBinding {
            address: vendor.symbol(WIRE_QUERY)?,
            boundary: vendor.boundary(WIRE_QUERY),
            allow_tail: false,
        },
        applicability: "the external coexistence wire type the case selects".into(),
        ..model("coex-external-wire", 0, wire)
    })
}

/// `hal_set_extern_pti_mode(mode)`: the probe's wire answers the vendor's
/// follower query.
fn external_mode_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [mode, wire] = *words else {
        unreachable!("mode words: mode, wire")
    };
    Ok(Objects {
        vendor_words: vec![mode],
        calls: vec![wire_model(vendor, wire)?],
        ..Default::default()
    })
}

/// The vendor's retained follower flag, a static byte of
/// `hal_external_coexist.o` without a symbol, addressed by its section.
const FOLLOWER_FLAG: &str = ".bss.s_external_coex_is_slv_mode";

/// The vendor keeps the work mode in its follower flag; production takes
/// the mode with each operation. Each case seeds the flag for its mode.
fn follower_flag(vendor: &Vendor<'_>, mode: u32) -> Result<(u32, Vec<u8>)> {
    Ok((
        vendor.image_symbol(FOLLOWER_FLAG)?,
        vec![u8::from(mode == FOLLOWER_MODE)],
    ))
}

/// `hal_set_extern_pti(first, second, third)` in the case's mode.
fn external_pti_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [mode, wire, first, second, third] = *words else {
        unreachable!("priority words: mode, wire, three priorities")
    };
    Ok(Objects {
        vendor_words: vec![first, second, third],
        image: vec![follower_flag(vendor, mode)?],
        calls: vec![wire_model(vendor, wire)?],
        ..Default::default()
    })
}

/// `hal_clr_extern_pti()` in the case's mode.
fn external_clear_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [mode] = *words else {
        unreachable!("clear words: mode")
    };
    Ok(Objects {
        image: vec![follower_flag(vendor, mode)?],
        ..Default::default()
    })
}

/// `ic_set_extern_coex_params(mode, {delay, validity})`: the parameter
/// structure travels by value in one argument word.
fn external_params_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [mode, wire, delay, validity] = *words else {
        unreachable!("parameter words: mode, wire, delay, validity")
    };
    Ok(Objects {
        vendor_words: vec![mode, delay | validity << 8],
        calls: vec![wire_model(vendor, wire)?],
        ..Default::default()
    })
}

/// Production borrows the timer bank through a validation lease of the
/// radio arbiter and releases it with one release fence after the leaf's
/// register transaction.
const RELEASE_FENCES: u32 = 1;

/// Session input of `libbtbb.a`, the suite's second archive.
const BTBB_INPUT: u64 = 4;

/// Every compared coexistence leaf.
pub const LEAVES: &[Leaf] = &[
    released(
        leaf(
            "coex_hw_timer_enable",
            "open_coex_trace_coex_hw_timer_enable",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    released(
        leaf(
            "coex_hw_timer_disable",
            "open_coex_trace_coex_hw_timer_disable",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    released(
        leaf(
            "coex_hw_timer_force",
            "open_coex_trace_coex_hw_timer_force",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    released(
        leaf(
            "coex_hw_timer_unforce",
            "open_coex_trace_coex_hw_timer_unforce",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    // The vendor reads its priority table without a bound, so the domain
    // is the admitted events.
    leaf(
        "coex_core_pti_get",
        "open_coex_core_trace_pti_get",
        &[("event", Domain::Words(EVENTS)), ("output", output(1))],
        true,
    ),
    leaf(
        "coex_core_event_duration_get",
        "open_coex_core_trace_event_duration_get",
        &[
            ("event", Domain::Words(EVENTS_AND_INVALID)),
            ("output", output(4)),
        ],
        true,
    ),
    leaf(
        "coex_core_timer_idx_get",
        "open_coex_core_trace_timer_idx_get",
        &[("event", Domain::Words(DRIVER_EVENTS_AND_INVALID))],
        true,
    ),
    // An event without a policy timer is rejected before the lease.
    released_when_leased(
        leaf(
            "coex_core_release",
            "open_coex_core_trace_release",
            &[
                ("_client", Domain::Words(&[0, 1])),
                ("event", Domain::Words(DRIVER_EVENTS_AND_INVALID)),
            ],
            true,
        ),
        RELEASE_FENCES,
    ),
    // The timer clock `coex_core_pre_init` selects (4 over 50 on silicon, 8
    // over 1 otherwise), every selector's lowest accepted and rejected
    // divisor, a non-one-hot selector and a divisor past the twelve-bit field.
    // A rejected selection is refused before the lease.
    released_when_leased(
        stated(
            objects(
                leaf(
                    "coex_hw_timer_freq_set",
                    "open_coex_trace_timer_freq_set",
                    &[
                        ("selector", Domain::Words(&[1, 2, 3, 4, 8])),
                        ("divisor", Domain::Words(&[1, 2, 3, 39, 40, 49, 50, 4_097])),
                    ],
                    true,
                ),
                freq_set_abi,
            ),
            CLOCK_SELECTIONS,
        ),
        RELEASE_FENCES,
    ),
    // Clients 0 and 1 are the request kinds production admits, as
    // Bluetooth and Wi-Fi.
    released(
        stated(
            objects(
                leaf(
                    "coex_hw_timer_set",
                    "open_coex_set_trace_coex_hw_timer_set",
                    &[
                        ("index", Domain::Words(TIMERS)),
                        ("client", Domain::Words(&[0, 1])),
                        ("pti", Domain::Words(&[0, 0x0f])),
                        ("latency", Domain::Words(&[0, 2_000])),
                        ("duration", Domain::Words(&[3_000, u32::MAX])),
                        ("is_real_chip", Domain::Words(REAL_CHIP)),
                    ],
                    false,
                ),
                timer_set_abi,
            ),
            CLOCK_SELECTIONS,
        ),
        RELEASE_FENCES,
    ),
    released_when_leased(
        stated(
            objects(
                leaf(
                    "coex_core_request",
                    "open_coex_core_trace_request",
                    &[
                        ("client", Domain::Words(&[0, 1])),
                        ("event", Domain::Words(DRIVER_EVENTS_AND_INVALID)),
                        ("latency", Domain::Words(&[2_000])),
                        ("duration", Domain::Words(&[3_000])),
                        ("is_real_chip", Domain::Words(REAL_CHIP)),
                    ],
                    true,
                ),
                request_abi,
            ),
            CLOCK_SELECTIONS,
        ),
        RELEASE_FENCES,
    ),
    // The arbiter's PHY grant protect on event 48 and timer 5: production's
    // `PhyGrantProtect` keeps its validation lease.
    vendor_reads(
        stated(
            objects(
                leaf(
                    "phy_acquire_grant_protect",
                    "open_coex_trace_phy_acquire_grant_protect",
                    &[],
                    true,
                ),
                grant_abi,
            ),
            CLOCK_SELECTIONS,
        ),
        GRANT_CLOCK_READ,
    ),
    vendor_reads(
        stated(
            objects(
                leaf(
                    "phy_release_grant_protect",
                    "open_coex_trace_phy_release_grant_protect",
                    &[],
                    true,
                ),
                grant_abi,
            ),
            CLOCK_SELECTIONS,
        ),
        GRANT_CLOCK_READ,
    ),
    // IEEE 802.15.4 coexistence priorities.
    // The level's priority comes from the local `coex_ieee802154_pti_get`.
    leaf(
        "esp_coex_ieee802154_txrx_pti_set",
        "open_ieee802154_coex_trace_txrx_level",
        &[("level", Domain::Words(IEEE802154_LEVELS))],
        false,
    ),
    leaf(
        "esp_coex_ieee802154_ack_pti_set",
        "open_ieee802154_coex_trace_ack_level",
        &[("level", Domain::Words(IEEE802154_LEVELS))],
        false,
    ),
    leaf(
        "hal_set_IEEE802154_TXRX_pti",
        "open_ieee802154_coex_trace_txrx_pti",
        &[("pti", Domain::Words(PTIS))],
        false,
    ),
    leaf(
        "hal_set_IEEE802154_ACK_pti",
        "open_ieee802154_coex_trace_ack_pti",
        &[("pti", Domain::Words(PTIS))],
        false,
    ),
    // External coexistence in leader mode, and the work-mode selection.
    objects(
        leaf(
            "hal_set_extern_pti_mode",
            "open_coex_external_trace_pti_mode",
            &[
                ("mode", Domain::Words(EXTERNAL_MODES)),
                ("wire", Domain::Words(EXTERNAL_WIRES)),
            ],
            false,
        ),
        external_mode_abi,
    ),
    objects(
        leaf(
            "hal_set_extern_pti",
            "open_coex_external_trace_set_pti",
            &[
                ("mode", Domain::Words(EXTERNAL_MODES)),
                ("wire", Domain::Words(EXTERNAL_WIRES)),
                ("first", Domain::Words(EXTERNAL_PTIS)),
                ("second", Domain::Words(EXTERNAL_PTIS)),
                ("third", Domain::Words(EXTERNAL_PTIS)),
            ],
            false,
        ),
        external_pti_abi,
    ),
    objects(
        leaf(
            "hal_clr_extern_pti",
            "open_coex_external_trace_clear_pti",
            &[("mode", Domain::Words(EXTERNAL_MODES))],
            false,
        ),
        external_clear_abi,
    ),
    leaf(
        "hal_enable_extern_coex",
        "open_coex_external_trace_enable",
        &[],
        false,
    ),
    leaf(
        "hal_disable_extern_coex",
        "open_coex_external_trace_disable",
        &[],
        false,
    ),
    objects(
        leaf(
            "ic_set_extern_coex_params",
            "open_coex_external_trace_params",
            &[
                ("mode", Domain::Words(EXTERNAL_MODES)),
                ("wire", Domain::Words(EXTERNAL_WIRES)),
                ("delay_us", Domain::Words(EXTERNAL_DELAYS)),
                ("validate_high", Domain::Words(EXTERNAL_VALIDITY)),
            ],
            true,
        ),
        external_params_abi,
    ),
    // The IEEE 802.15.4 MAC TX-on, TX-off and TX/RX switch delays.
    in_archive(
        leaf(
            "ieee802154_txon_delay_set",
            "open_ieee802154_btbb_trace_txon_delay",
            &[],
            true,
        ),
        BTBB_INPUT,
    ),
];

/// The coexistence hardware suite.
pub const COEX_HW: Suite = Suite {
    title: "Coexistence hardware leaf comparison",
    archives: &["libcoexist", "libbtbb"],
    leaves: LEAVES,
    id: crate::mac::CONTRACT_ID,
    rom: crate::mac::ROM,
    firmware: Some(crate::mac::FIRMWARE),
    roots: &[],
    prepare: None,
    claims: &[],
    // ESP-IDF glue the request's invalid-kind assertion reaches; the compared
    // kinds never do.
    absent: &["coexist_printf"],
};
