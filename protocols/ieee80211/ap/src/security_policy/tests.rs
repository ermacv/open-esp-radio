use super::*;

#[test]
fn the_personal_profiles_advertise_their_recorded_elements() {
    assert_eq!(
        AP_WPA2_PERSONAL_RSN_ELEMENT,
        [
            0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
        ]
    );
    assert_eq!(
        AP_WPA3_PERSONAL_RSN_ELEMENT,
        [
            0x30, 26, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 8, 0xc0,
            0, 0, 0, 0, 0x0f, 0xac, 6,
        ]
    );
    assert_eq!(AP_SAE_H2E_RSNX_ELEMENT, [244, 1, 0x20]);
    assert_eq!(
        ApSecurityPolicy::Open.advertisement(),
        BssSecurityElements::OPEN
    );
    for policy in [
        ApSecurityPolicy::Wpa2Personal,
        ApSecurityPolicy::Wpa3Personal,
    ] {
        assert_eq!(
            policy.advertisement().link_protection(),
            policy.link_protection()
        );
    }
}
