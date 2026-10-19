//! The diagnostic vendor command that armed one received-MIC corruption:
//! its opcode, its Command Complete and the corruption itself.
//!
//! Vendor-specific HCI command `0xFC01` (OGF `0x3F`, OCF `0x001`) took one
//! Connection_Handle and could be sent before encryption started. The next
//! received encrypted data PDU on that connection had one MIC octet inverted
//! before CCM authentication; plaintext PDUs carry no MIC and were left
//! alone, so the production authentication failure and connection exit ran
//! unchanged. The one-shot arming per connection, its clearing on
//! disconnection and Reset, and the choice of the Command Complete status
//! (`0x00`, `0x02` Unknown Connection Identifier, `0x0C` Command Disallowed
//! while the connection was closing, `0x12` for malformed parameters) belong
//! to a Controller core, which this repository does not have yet.

use bt_hci::{
    cmd::{Opcode, OpcodeGroup},
    param::Status,
};

/// Arm one received-MIC corruption.
pub const ARM_MIC_CORRUPTION: Opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 0x0001);

/// Command Complete of [`ARM_MIC_CORRUPTION`] with its status and the
/// Connection_Handle octets the Host sent.
pub fn command_complete(status: Status, handle: [u8; 2]) -> [u8; 8] {
    let opcode = ARM_MIC_CORRUPTION.to_raw().to_le_bytes();
    [
        0x0e,
        6,
        1,
        opcode[0],
        opcode[1],
        status.into_inner(),
        handle[0],
        handle[1],
    ]
}

/// Copy `pdu` into `corrupted` with its last MIC octet inverted when it is an
/// encrypted data PDU, which carries a MIC: a data LLID and a non-empty
/// payload. Otherwise nothing is corrupted and `None` is returned.
pub fn corrupt_data_mic<'a>(pdu: &[u8], corrupted: &'a mut [u8]) -> Option<&'a [u8]> {
    let (&header, rest) = pdu.split_first()?;
    let (&length, _) = rest.split_first()?;
    let data = matches!(header & 0x03, 0x01 | 0x02);
    if !data || length == 0 || pdu.len() > corrupted.len() {
        return None;
    }
    let corrupted = &mut corrupted[..pdu.len()];
    corrupted.copy_from_slice(pdu);
    *corrupted.last_mut()? ^= 0xff;
    Some(corrupted)
}

#[cfg(test)]
mod tests;
