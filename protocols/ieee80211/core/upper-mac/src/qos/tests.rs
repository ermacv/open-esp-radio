use super::*;
use oer_ieee80211_wire::extensions::wmm::WmmAcParameters;

/// The standard default EDCA set with ACM set on `restricted`.
fn edca(restricted: &[WmmAccessCategory]) -> WmmParameterSet {
    let defaults = WmmParameterSet::STANDARD_DEFAULT;
    let record = |category: WmmAccessCategory| WmmAcParameters {
        admission_control_mandatory: restricted.contains(&category),
        ..defaults.access_category(category)
    };
    WmmParameterSet::new(
        1,
        false,
        [
            record(WmmAccessCategory::BestEffort),
            record(WmmAccessCategory::Background),
            record(WmmAccessCategory::Video),
            record(WmmAccessCategory::Voice),
        ],
    )
}

#[test]
fn an_open_category_keeps_the_requested_priority() {
    let admission = AcAdmission::from_edca(edca(&[]));
    assert_eq!(admission, AcAdmission::OPEN);
    for priority in WmmUserPriority::ALL {
        assert_eq!(admission.admitted_user_priority(priority), Some(priority));
    }
}

#[test]
fn a_closed_category_takes_the_next_lower_open_one_at_its_canonical_priority() {
    let voice = AcAdmission::from_edca(edca(&[WmmAccessCategory::Voice]));
    assert!(voice.restricts(WmmAccessCategory::Voice));
    assert!(!voice.restricts(WmmAccessCategory::Video));
    for requested in [WmmUserPriority::UP6, WmmUserPriority::UP7] {
        assert_eq!(
            voice.admitted_user_priority(requested),
            Some(WmmUserPriority::UP5)
        );
    }
    // A priority of an open category is never changed.
    assert_eq!(
        voice.admitted_user_priority(WmmUserPriority::UP4),
        Some(WmmUserPriority::UP4)
    );

    let voice_and_video =
        AcAdmission::from_edca(edca(&[WmmAccessCategory::Voice, WmmAccessCategory::Video]));
    assert_eq!(
        voice_and_video.admitted_user_priority(WmmUserPriority::UP7),
        Some(WmmUserPriority::UP0)
    );
    assert_eq!(
        voice_and_video.admitted_user_priority(WmmUserPriority::UP4),
        Some(WmmUserPriority::UP0)
    );
}

#[test]
fn no_open_category_at_or_below_the_request_leaves_no_priority() {
    let all = AcAdmission::from_edca(edca(&WmmAccessCategory::ALL));
    for priority in WmmUserPriority::ALL {
        assert_eq!(all.admitted_user_priority(priority), None);
    }
    // Background open below a closed best effort.
    let best_effort = AcAdmission::from_edca(edca(&[WmmAccessCategory::BestEffort]));
    assert_eq!(
        best_effort.admitted_user_priority(WmmUserPriority::UP0),
        Some(WmmUserPriority::UP1)
    );
    let background = AcAdmission::from_edca(edca(&[WmmAccessCategory::Background]));
    assert_eq!(
        background.admitted_user_priority(WmmUserPriority::UP2),
        None
    );
}
