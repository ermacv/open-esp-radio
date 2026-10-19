use super::model::{Config, Request};

/// Radiotap RATE=1 Mbit/s, followed by a directed-SSID broadcast probe request.
/// The synthetic source is never assigned to a VIF, so it must not auto-ACK
/// responses. Air evidence must confirm actual injection and absence of retries.
pub fn encode(config: &Config, bssid: [u8; 6], request: Request) -> Vec<u8> {
    let mut frame = vec![0, 0, 9, 0, 4, 0, 0, 0, 2];
    frame.extend_from_slice(&[0x40, 0, 0, 0]);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&request.source);
    frame.extend_from_slice(&bssid);
    frame.extend_from_slice(&(request.sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0, config.ssid.len() as u8]);
    frame.extend_from_slice(config.ssid.as_bytes());
    frame.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    frame.extend_from_slice(&[3, 1, config.channel]);
    frame
}
