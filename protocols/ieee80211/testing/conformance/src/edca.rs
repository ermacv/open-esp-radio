//! EDCA parameter tables a backend fixture installs: distinct complete
//! tables per interface, so a fixture can tell which interface's table a
//! publication contended with.

use oer_protocols_ieee80211_contracts_port::{EdcaParameterSet, VifRole};
use oer_protocols_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};

/// Distinct complete tables for fixture input; records are in ACI order.
pub const fn parameters(role: VifRole, updated: bool) -> EdcaParameterSet {
    let (aifsn, minimum, maximum, txop, lifetime) = match (role, updated) {
        (VifRole::Station, _) => (
            oer_portable_wire::U8::constant::<3>(),
            oer_portable_wire::U8::constant::<4>(),
            oer_portable_wire::U8::constant::<6>(),
            94,
            500,
        ),
        (VifRole::AccessPoint, false) => (
            oer_portable_wire::U8::constant::<7>(),
            oer_portable_wire::U8::constant::<2>(),
            oer_portable_wire::U8::constant::<3>(),
            47,
            200,
        ),
        (VifRole::AccessPoint, true) => (
            oer_portable_wire::U8::constant::<5>(),
            oer_portable_wire::U8::constant::<1>(),
            oer_portable_wire::U8::constant::<2>(),
            350,
            100,
        ),
    };
    EdcaParameterSet::from_tu(
        WmmParameterSet::new(
            0,
            false,
            [WmmAcParameters {
                admission_control_mandatory: false,
                aifsn,
                ecw_min: minimum,
                ecw_max: maximum,
                txop_limit_units_32_us: txop,
            }; 4],
        ),
        [lifetime; 4],
    )
}
