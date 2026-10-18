#[cfg(any(feature = "diagnostics", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EthernetProtocol {
    ArpRequest,
    ArpReply,
    Ipv4Tcp,
    Ipv4Other,
    Other,
}

#[cfg(any(feature = "diagnostics", test))]
fn ethernet_protocol(frame: &[u8]) -> Option<EthernetProtocol> {
    let ether_type = u16::from_be_bytes([*frame.get(12)?, *frame.get(13)?]);
    match ether_type {
        0x0800 => Some(if *frame.get(23)? == 6 {
            EthernetProtocol::Ipv4Tcp
        } else {
            EthernetProtocol::Ipv4Other
        }),
        0x0806 => match u16::from_be_bytes([*frame.get(20)?, *frame.get(21)?]) {
            1 => Some(EthernetProtocol::ArpRequest),
            2 => Some(EthernetProtocol::ArpReply),
            _ => Some(EthernetProtocol::Other),
        },
        _ => Some(EthernetProtocol::Other),
    }
}

#[cfg(any(feature = "diagnostics", test))]
#[inline(always)]
fn ethernet_parts_protocol(frame: EthernetFrameParts<'_>) -> Option<EthernetProtocol> {
    match frame.ether_type {
        0x0800 => Some(if *frame.payload.get(9)? == 6 {
            EthernetProtocol::Ipv4Tcp
        } else {
            EthernetProtocol::Ipv4Other
        }),
        0x0806 => match u16::from_be_bytes(frame.payload.get(6..8)?.try_into().ok()?) {
            1 => Some(EthernetProtocol::ArpRequest),
            2 => Some(EthernetProtocol::ArpReply),
            _ => Some(EthernetProtocol::Other),
        },
        _ => Some(EthernetProtocol::Other),
    }
}
