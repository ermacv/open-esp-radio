use super::*;

fn round_trip(frame: &Frame) -> Frame {
    let mut wire = Vec::new();
    frame.write(&mut wire).unwrap();
    let mut input = wire.as_slice();
    let read = Frame::read(&mut input).unwrap().unwrap();
    assert!(Frame::read(&mut input).unwrap().is_none());
    read
}

#[test]
fn messages_and_data_cross_the_wire_unchanged() {
    let message = Frame::message(&Request::Hello {
        protocol: PROTOCOL,
        client: "test".to_owned(),
    })
    .unwrap();
    assert_eq!(round_trip(&message), message);
    let data = Frame::Data {
        operation: u64::MAX,
        bytes: b"console".to_vec(),
    };
    assert_eq!(round_trip(&data), data);
}

#[test]
fn a_frame_beyond_the_limit_is_neither_sent_nor_read() {
    let large = Frame::Data {
        operation: 1,
        bytes: vec![0; MAX_FRAME],
    };
    assert!(large.write(&mut Vec::new()).is_err());
    let mut wire = u32::try_from(MAX_FRAME + 1).unwrap().to_be_bytes().to_vec();
    wire.push(0);
    assert_eq!(
        Frame::read(&mut wire.as_slice()).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn a_truncated_or_unknown_frame_is_an_error() {
    let mut wire = Vec::new();
    Frame::Message(b"{}".to_vec()).write(&mut wire).unwrap();
    wire.pop();
    assert!(Frame::read(&mut wire.as_slice()).is_err());
    let unknown = [0, 0, 0, 1, 9];
    assert!(Frame::read(&mut unknown.as_slice()).is_err());
    let short_data = [0, 0, 0, 3, DATA, 0, 0];
    assert!(Frame::read(&mut short_data.as_slice()).is_err());
}

#[test]
fn a_station_profile_without_a_width_is_twenty_megahertz() {
    let older = r#"{"router":"apply-station-profile","ssid":"s","passphrase":"p","channel":13}"#;
    let operation: RouterOperation = serde_json::from_str(older).unwrap();
    assert!(matches!(
        operation,
        RouterOperation::ApplyStationProfile {
            width: RouterChannelWidth::Ht20,
            ..
        }
    ));
}

#[test]
fn a_station_link_names_its_station() {
    let operation = RouterOperation::StationLink {
        mac: "02:00:00:00:00:01".to_owned(),
    };
    let text = serde_json::to_string(&operation).unwrap();
    assert_eq!(
        text,
        r#"{"router":"station-link","mac":"02:00:00:00:00:01"}"#
    );
    assert_eq!(
        serde_json::from_str::<RouterOperation>(&text).unwrap(),
        operation
    );
}
