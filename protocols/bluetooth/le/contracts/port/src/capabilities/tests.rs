use super::{LePhy, LePhys, LegacyAdvertisingTypes};
use crate::TestPhy;

#[test]
fn phy_sets_hold_what_they_were_built_with() {
    assert!(LePhys::LE_1M.contains(LePhy::Le1M));
    assert!(!LePhys::LE_1M.contains(LePhy::Le2M));
    assert!(!LePhys::NONE.contains(LePhy::Le1M));
    for phy in [LePhy::Le1M, LePhy::Le2M, LePhy::LeCoded] {
        assert!(LePhys::ALL.contains(phy));
    }
    assert_eq!(TestPhy::LeCodedS2.phy(), LePhy::LeCoded);
    assert_eq!(TestPhy::LeCodedS8.phy(), LePhy::LeCoded);
}

#[test]
fn advertising_types_serve_any_one_of_them() {
    assert!(!LegacyAdvertisingTypes::NONE.any());
    assert!(
        LegacyAdvertisingTypes {
            nonconnectable: false,
            responsive: true,
        }
        .any()
    );
}
