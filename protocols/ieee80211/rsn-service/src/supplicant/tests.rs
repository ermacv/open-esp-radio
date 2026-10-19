use core::future::{Future, ready};

use oer_ieee80211_mac::security::rsn::Akm;
use oer_ieee80211_rsn::{
    Pmk, PtkContext, RsnInterface,
    aes::{RsnSoftwareAes, RsnUnwrappedKeyData, software_aes128_key_wrap},
    frames::{OwnedRsnIe, RsnGtk, RsnPlainKeyData, RsnTxFrame},
    state::RsnStaPhase,
};

use super::*;

const LOCAL: [u8; 6] = [1; 6];
const AP: [u8; 6] = [2; 6];
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

fn owned(frame: &RsnTxFrame<512>) -> OwnedEapolFrame<512> {
    OwnedEapolFrame::try_copy(RsnInterface::Station, AP, frame.as_bytes()).unwrap()
}

/// A supplicant past Message 1 and the encrypted Message 3 it now awaits.
fn awaiting_message3(pmk: &Pmk) -> (RsnStaSupplicant, OwnedEapolFrame<512>) {
    let ptk = pmk.derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: LOCAL,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let mut supplicant = RsnStaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    let RsnStaSupplicantAction::Ready(RsnStaReady::Transmit(_)) =
        supplicant.on_frame(owned(&message1), pmk).unwrap()
    else {
        panic!("Message 1 must produce Message 2")
    };
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        false,
        &[0x5a; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes()).unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(&ptk);
    (supplicant, owned(&message3))
}

struct FailingUnwrap;

impl AsyncRsnKeyUnwrap for FailingUnwrap {
    type Error = u8;

    fn unwrap_key_data<'a>(
        &'a mut self,
        _kek: &'a [u8; 16],
        _encrypted: &'a [u8],
    ) -> impl Future<Output = Result<RsnUnwrappedKeyData, Self::Error>> + 'a {
        ready(Err(7))
    }
}

#[test]
fn awaited_unwrap_turns_message3_into_one_key_install() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let (mut supplicant, message3) = awaiting_message3(&pmk);
    let action = embassy_futures::block_on(process_frame(
        &mut supplicant,
        message3,
        &pmk,
        &mut RsnSoftwareAes::new(),
    ));
    let Ok(RsnStaReady::InstallKeys(request)) = action else {
        panic!("an encrypted Message 3 must resolve to its key install")
    };
    assert_eq!(request.replay_counter(), 2);
    assert!(request.encrypted_key_data());
    assert_eq!(supplicant.phase(), RsnStaPhase::InstallingKeys);
}

#[test]
fn failed_awaited_unwrap_rejects_the_key_data() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let (mut supplicant, message3) = awaiting_message3(&pmk);
    let action = embassy_futures::block_on(process_frame(
        &mut supplicant,
        message3,
        &pmk,
        &mut FailingUnwrap,
    ));
    assert!(matches!(action, Err(RsnStaProcessError::KeyUnwrap(7))));
    assert_eq!(supplicant.phase(), RsnStaPhase::Failed);
}
