use super::*;
use oer_bluetooth_le_radio::LeTxCeiling;
use oer_radio_regulatory::{Country, FrequencySpan, RegulatoryDomain, Rule, RuleFlags};

fn band(passive: bool) -> Permitted<LeBand> {
    extern crate std;
    let rules = std::boxed::Box::leak(std::boxed::Box::new([Rule::new(
        FrequencySpan::new(2_400_000, 2_500_000).expect("test band"),
        2_000,
        2000,
        if passive {
            RuleFlags::NO_IR
        } else {
            RuleFlags::NONE
        },
    )]));
    RegulatoryDomain::new(Country::new(*b"ZZ").expect("test country"), rules)
        .expect("test domain")
        .permit(LeBand)
        .expect("permitted test band")
}

fn limits(cap: i32) -> PowerLimits {
    PowerLimits {
        antenna_gain_mbm: 250,
        chip_maximum_mbm: 2100,
        owner_cap_mbm: Some(cap),
    }
}

fn requested(band: Permitted<LeBand>, dbm: i8) -> TxPower {
    TxPower::within(LeTxCeiling::new(band, band.tx_ceiling(limits(2000))), dbm)
        .expect("test request")
}

#[test]
fn passive_permission_refuses_every_operation_that_transmits() {
    let permission = Permission::new(band(true), limits(1000));
    for operation in [
        Operation::Advertising,
        Operation::ActiveScan,
        Operation::CreateConnection,
        Operation::Connection,
        Operation::DtmTx,
    ] {
        assert_eq!(permission.authorize(operation), Err(Refusal::PassiveOnly));
        assert_eq!(
            permission.tx_power(operation, requested(band(false), 0)),
            Err(Refusal::PassiveOnly)
        );
    }
    assert_eq!(permission.authorize(Operation::PassiveScan), Ok(()));
    assert_eq!(permission.authorize(Operation::DtmRx), Ok(()));
    for operation in [
        Operation::Advertising,
        Operation::ActiveScan,
        Operation::CreateConnection,
        Operation::DtmTx,
    ] {
        assert_eq!(
            BluetoothJoin::new(permission, operation, Some(requested(band(false), 0))),
            Err(Refusal::PassiveOnly)
        );
    }
    assert!(BluetoothJoin::new(permission, Operation::DtmRx, None).is_ok());
}

#[test]
fn initial_transmission_needs_explicit_power_and_installs_the_lower_provider_level() {
    let band = band(false);
    let permission = Permission::new(band, limits(975));
    assert_eq!(
        BluetoothJoin::new(permission, Operation::DtmRx, Some(requested(band, 0))),
        Err(Refusal::UnexpectedPower)
    );
    assert_eq!(
        BluetoothJoin::new(permission, Operation::DtmTx, None),
        Err(Refusal::PowerRequired)
    );
    assert_eq!(
        BluetoothJoin::new(permission, Operation::DtmTx, Some(requested(band, 10))),
        Err(Refusal::AboveCeiling)
    );
    let join = BluetoothJoin::new(permission, Operation::DtmTx, Some(requested(band, 9)))
        .expect("bounded initial TX");
    assert_eq!(join.operation(), Operation::DtmTx);
    assert_eq!(join.permission(), permission);
    assert!(join.power().expect("explicit TX power").level_dbm() <= 9);
}

#[test]
fn power_uses_the_exact_band_antenna_chip_and_owner_ceiling() {
    let band = band(false);
    assert_eq!(Permission::new(band, limits(3000)).ceiling().mbm(), 1750);
    assert_eq!(Permission::new(band, limits(975)).ceiling().mbm(), 975);
    let permission = Permission::new(band, limits(975));
    assert_eq!(
        permission.tx_power(Operation::DtmTx, requested(band, 10)),
        Err(Refusal::AboveCeiling)
    );
    let rounded = permission
        .tx_power(Operation::DtmTx, requested(band, 9))
        .expect("provider covers request");
    assert!(rounded.level_dbm() <= 9);
    assert!(i32::from(rounded.level_dbm()) * 100 <= permission.ceiling().mbm());
}

#[test]
fn provider_rounding_never_increases_even_a_negative_request() {
    let band = band(false);
    let permission = Permission::new(band, limits(-150));
    for dbm in LeTxPower::LOWEST_DBM..=-2 {
        let power = permission
            .tx_power(Operation::Advertising, requested(band, dbm))
            .expect("covered provider level");
        assert!(power.level_dbm() <= dbm);
    }
    assert_eq!(
        permission.tx_power(
            Operation::Advertising,
            requested(band, LeTxPower::LOWEST_DBM - 1)
        ),
        Err(Refusal::BelowProviderMinimum)
    );
    assert_eq!(
        permission.tx_power(Operation::Advertising, requested(band, -1)),
        Err(Refusal::AboveCeiling)
    );
}
