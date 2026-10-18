//! Authenticate received data channel PDUs before Link Layer and Host
//! dispatch.
//!
//! Empty PDUs carry no MIC and never reach the Host; they advance neither the
//! encryption handshake nor a packet counter (Core Vol 6 Part B 2.4.1 and
//! 5.1.3). The encryption procedure's receive mode decides how every other
//! packet is read; a packet that mode forbids fails the procedure, which then
//! requires the connection to terminate.

use oer_bluetooth_ll::security::{
    LePeripheralEncryptedReceive as Received, LePeripheralEncryptionProcedure as Procedure,
    LePeripheralEncryptionProcedureError as Error, LePeripheralEncryptionRandom,
    LePeripheralEncryptionReceiveMode as Mode,
};

/// Header, payload and MIC of the longest data channel PDU.
pub(crate) const PDU_CAPACITY: usize = u8::MAX as usize + 2;

/// The plaintext PDU, header included, to dispatch next; `None` when the
/// packet was consumed by the encryption procedure, was empty, or failed it.
// CAPABILITY: bluetooth-ll-data-pdu-rx
pub(crate) fn decode<'a>(
    encryption: &mut Procedure,
    pdu: &[u8],
    decoded: &'a mut [u8; PDU_CAPACITY],
    random: impl FnMut() -> Option<LePeripheralEncryptionRandom>,
) -> Option<&'a [u8]> {
    // A failed procedure stays failed for the rest of the event.
    if encryption.termination().is_some() {
        return None;
    }
    match decode_inner(encryption, pdu, decoded, random) {
        Ok(length) => length.map(|length| &decoded[..length]),
        Err(_) => {
            decoded.fill(0);
            encryption.fail_unexpected_physical_channel_pdu();
            None
        }
    }
}

fn decode_inner(
    encryption: &mut Procedure,
    pdu: &[u8],
    decoded: &mut [u8; PDU_CAPACITY],
    mut random: impl FnMut() -> Option<LePeripheralEncryptionRandom>,
) -> Result<Option<usize>, Error> {
    if pdu.len() < 2 || usize::from(pdu[1]) + 2 != pdu.len() {
        return Err(Error::UnexpectedPhysicalChannelPdu);
    }
    let header = pdu[0];
    let payload = &pdu[2..];
    if header & 0x03 == 0x01 && payload.is_empty() {
        return Ok(None);
    }
    decoded[..pdu.len()].copy_from_slice(pdu);
    match encryption.receive_mode() {
        Mode::Plaintext => {
            if header & 0x03 == 0x03 && payload.first() == Some(&0x03) {
                if payload.len() != 23 {
                    return Err(Error::MalformedEncryptionRequest);
                }
                encryption.begin(payload, random().ok_or(Error::InvalidState)?)?;
            } else {
                return Ok(Some(pdu.len()));
            }
        }
        Mode::EncryptedStartResponse => {
            encryption.receive_encrypted_start_response(header, &mut decoded[2..pdu.len()])?;
        }
        Mode::Encrypted => {
            match encryption.receive_active_packet(header, &mut decoded[2..pdu.len()])? {
                Received::PauseRequest => {}
                Received::Plaintext { length } => {
                    decoded[1] = length as u8;
                    return Ok(Some(length + 2));
                }
            }
        }
        Mode::UnencryptedPauseResponse => {
            if header & 0x03 != 0x03 {
                return Err(Error::UnexpectedPhysicalChannelPdu);
            }
            encryption.receive_unencrypted_pause_response(payload)?;
        }
        Mode::RestartEncryptionRequest => {
            if header & 0x03 != 0x03 || payload.len() != 23 || payload[0] != 0x03 {
                return Err(Error::UnexpectedPhysicalChannelPdu);
            }
            encryption.begin(payload, random().ok_or(Error::InvalidState)?)?;
        }
        Mode::Blocked => return Err(Error::UnexpectedPhysicalChannelPdu),
    }
    Ok(None)
}
