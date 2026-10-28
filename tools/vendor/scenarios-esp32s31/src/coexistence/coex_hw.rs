//! Coexistence hardware-timer and core-release leaves of the pinned
//! `libcoexist.a`, compared with the compiled production coexistence probes
//! on the `wifi-mac` leaf machinery, over the probe image's session.
use crate::harness::Result;
use crate::mac::{Domain, Leaf, Objects, Suite, Vendor, leaf, objects, stated};

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
/// timers. The driver leaves compare every other event.
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

/// A nullable output object of `length` bytes.
const fn output(length: u32) -> Domain {
    Domain::Output {
        offset: 0,
        length,
        nullable: true,
    }
}

// SOURCE: firmware/probes/esp32s31-radio/src/coex.rs::with_domain lends
// RadioHardware::for_probe().split(()) directly; hal/src/coex.rs::CoexPolicyTimerBank
// borrows those owned registers without a lease or a release fence. These
// leaves compare all effects exactly, including any unexpected fence.

/// Every compared coexistence leaf.
pub const LEAVES: &[Leaf] = &[
    leaf(
        "coex_hw_timer_enable",
        "open_coex_trace_coex_hw_timer_enable",
        &[("index", Domain::Words(TIMERS))],
        false,
    ),
    leaf(
        "coex_hw_timer_disable",
        "open_coex_trace_coex_hw_timer_disable",
        &[("index", Domain::Words(TIMERS))],
        false,
    ),
    leaf(
        "coex_hw_timer_force",
        "open_coex_trace_coex_hw_timer_force",
        &[("index", Domain::Words(TIMERS))],
        false,
    ),
    leaf(
        "coex_hw_timer_unforce",
        "open_coex_trace_coex_hw_timer_unforce",
        &[("index", Domain::Words(TIMERS))],
        false,
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
    // An event without a policy timer is rejected before accessing registers.
    leaf(
        "coex_core_release",
        "open_coex_core_trace_release",
        &[
            ("_client", Domain::Words(&[0, 1])),
            ("event", Domain::Words(DRIVER_EVENTS_AND_INVALID)),
        ],
        true,
    ),
    // The timer clock `coex_core_pre_init` selects (4 over 50 on silicon, 8
    // over 1 otherwise), every selector's lowest accepted and rejected
    // divisor, a non-one-hot selector and a divisor past the twelve-bit field.
    // A rejected selection is refused before accessing registers.
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
];

/// The coexistence hardware suite.
pub const COEX_HW: Suite = Suite {
    title: "Coexistence hardware leaf comparison",
    archives: &["libcoexist"],
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
    session: Some(crate::session_entry::ENTRY),
};
