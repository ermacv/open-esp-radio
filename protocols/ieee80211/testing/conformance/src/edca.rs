//! EDCA parameter tables a backend fixture installs: distinct complete
//! tables per interface, so a fixture can tell which interface's table a
//! publication contended with.

use oer_protocols_ieee80211_contracts_port::{EdcaParameterSet, VifRole};
use oer_protocols_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};

/// Which of an interface's tables: the one it installs first, or the one
/// an update installs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Table {
    /// The first.
    First,
    /// The update's.
    Updated,
}

/// Distinct complete tables for fixture input; records are in ACI order.
pub const fn parameters(role: VifRole, table: Table) -> EdcaParameterSet {
    let (aifsn, minimum, maximum, txop, lifetime) = match (role, table) {
        (VifRole::Station, _) => (
            oer_portable_wire::U8::constant::<3>(),
            oer_portable_wire::U8::constant::<4>(),
            oer_portable_wire::U8::constant::<6>(),
            94,
            500,
        ),
        (VifRole::AccessPoint, Table::First) => (
            oer_portable_wire::U8::constant::<7>(),
            oer_portable_wire::U8::constant::<2>(),
            oer_portable_wire::U8::constant::<3>(),
            47,
            200,
        ),
        (VifRole::AccessPoint, Table::Updated) => (
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
            oer_protocols_ieee80211_wire::extensions::wmm::Uapsd::NotSupported,
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
