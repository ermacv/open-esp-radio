use super::*;

#[derive(Default)]
struct Hardware {
    plans: std::vec::Vec<MacStaApReceivePlan>,
    disabled: std::vec::Vec<crate::MacInterface>,
    disabled_all: usize,
}

impl StaApRegisterHardware for Hardware {
    fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan) {
        self.plans.push(plan);
    }

    fn disable_station_receive_registers(&mut self) {
        self.disabled.push(crate::MacInterface::Station);
    }

    fn disable_access_point_receive_registers(&mut self) {
        self.disabled.push(crate::MacInterface::AccessPoint);
    }

    fn disable_all_role_receive_registers(&mut self) {
        self.disabled_all += 1;
    }
}

#[test]
fn lmac_publishes_the_combined_plan_as_one_operation() {
    let station_address = [0x02, 0, 0, 0, 0, 1];
    let station_bssid = [0x02, 0, 0, 0, 0, 2];
    let access_point_address = [0x02, 0, 0, 0, 0, 3];
    let plan = MacStaApReceivePlan::observed_mode_one(
        station_address,
        station_bssid,
        access_point_address,
    );
    let mut hardware = Hardware::default();

    configure_sta_ap_receive_registers(
        &mut hardware,
        station_address,
        station_bssid,
        access_point_address,
    );

    assert_eq!(hardware.plans.as_slice(), &[plan]);
}

#[test]
fn each_role_can_leave_without_reconfiguring_the_surviving_bank() {
    let mut hardware = Hardware::default();

    disable_station_receive_registers(&mut hardware);
    disable_access_point_receive_registers(&mut hardware);

    assert!(hardware.plans.is_empty());
    assert_eq!(
        hardware.disabled,
        [
            crate::MacInterface::Station,
            crate::MacInterface::AccessPoint
        ]
    );
}

#[test]
fn role_neutral_policy_is_one_register_operation() {
    let mut hardware = Hardware::default();

    disable_all_role_receive_registers(&mut hardware);

    assert!(hardware.plans.is_empty());
    assert!(hardware.disabled.is_empty());
    assert_eq!(hardware.disabled_all, 1);
}
