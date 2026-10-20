use super::*;

#[test]
fn vif_and_channel_context_are_distinct_domains() {
    let station = VirtualInterface::new(VifId::PRIMARY, VifRole::Station, [0x02, 0, 0, 0, 0, 1]);
    let access_point =
        VirtualInterface::new(VifId::new(1), VifRole::AccessPoint, [0x02, 0, 0, 0, 0, 2]);
    let station_binding = VifChannelBinding::new(station.id, ChannelContextId::PRIMARY);
    let access_point_binding = VifChannelBinding::new(access_point.id, ChannelContextId::PRIMARY);

    assert_ne!(station.id, access_point.id);
    assert_eq!(
        station_binding.channel_context,
        access_point_binding.channel_context
    );
    assert_eq!(
        BoundVirtualInterface::new(station, ChannelContextId::PRIMARY).binding(),
        station_binding
    );
}

#[test]
fn monitor_is_an_observation_point_not_a_vif_role() {
    assert_ne!(MonitorTapPoint::Raw, MonitorTapPoint::Normalized);
    assert_ne!(
        MonitorTapPoint::Normalized,
        MonitorTapPoint::ProtocolValidated
    );
}
