//! Station authentication, disconnect, deauthentication and action frame
//! codecs.

use super::*;
use crate::address::{Bssid, OwnAddress};

/// An Open System request distinguishes the transmitter from the BSS.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_mac::station::OpenAuthenticationRequest;
/// fn swapped(request: OpenAuthenticationRequest) -> OpenAuthenticationRequest {
///     OpenAuthenticationRequest {
///         source: request.bssid,
///         bssid: request.source,
///         sequence_number: request.sequence_number,
///     }
/// }
/// ```
///
/// ```
/// use oer_ieee80211_mac::station::OpenAuthenticationRequest;
/// fn unchanged(request: OpenAuthenticationRequest) -> OpenAuthenticationRequest {
///     OpenAuthenticationRequest {
///         source: request.source,
///         bssid: request.bssid,
///         sequence_number: request.sequence_number,
///     }
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenAuthenticationRequest {
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub sequence_number: SequenceNumber,
}

impl OpenAuthenticationRequest {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid.get())?;
        let required = MANAGEMENT_HEADER_LEN + 6;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            OPEN_AUTHENTICATION_FRAME_CONTROL,
            self.bssid.get(),
            self.source.get(),
            self.bssid.get(),
            self.sequence_number,
        );
        frame[24..26].copy_from_slice(&OPEN_SYSTEM_ALGORITHM.to_le_bytes());
        frame[26..28].copy_from_slice(&OPEN_SYSTEM_REQUEST_SEQUENCE.to_le_bytes());
        frame[28..30].copy_from_slice(&0_u16.to_le_bytes());
        Ok(required)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenAuthenticationResponse {
    pub status_code: u16,
}

/// Management transition by which the selected AP ends the STA relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectKind {
    Disassociation,
    Deauthentication,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDisconnect {
    pub kind: StaDisconnectKind,
    pub reason_code: u16,
}

/// Management subtype of a frame this station originates after association.
///
/// An Action frame carries a category-led body; a Deauthentication carries
/// its two-byte reason code. Both are robust under management frame
/// protection except the Action categories the standard exempts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaManagementSubtype {
    Action,
    Deauthentication,
}

impl StaManagementSubtype {
    const fn frame_control(self) -> u16 {
        match self {
            Self::Action => ACTION_FRAME_CONTROL,
            Self::Deauthentication => DEAUTHENTICATION_FRAME_CONTROL,
        }
    }
}

/// One unprotected STA-originated management frame.
///
/// BlockAck negotiation sends an Action whose nine-byte body the MAC
/// BlockAck state machine owns; a leaving station sends a Deauthentication
/// with its reason code.
///
/// `SOURCE[PROMOTED_RX_AMPDU]`: reviewed promoted ADDBA response builder,
/// where the same header was constructed around
/// `block_ack::write_successful_addba_response`; the frame-control subtypes
/// are the IEEE 802.11 management subtypes also parsed by `libnet80211.a`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaManagementFrame<'a> {
    pub subtype: StaManagementSubtype,
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub sequence_number: SequenceNumber,
    pub body: &'a [u8],
}

impl StaManagementFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid.get())?;
        let required = MANAGEMENT_HEADER_LEN.checked_add(self.body.len()).ok_or(
            StationFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            self.subtype.frame_control(),
            self.bssid.get(),
            self.source.get(),
            self.bssid.get(),
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..].copy_from_slice(self.body);
        Ok(required)
    }
}

/// One robust STA-originated management frame under the pairwise key of an
/// association that protects its management frames.
///
/// The frame carries the Protected bit and the CCMP header; hardware appends
/// the MIC, as for protected data. The vendor protects a robust management
/// frame through the same `ieee80211_crypto_encap` key selection as data.
///
/// SOURCE(esp32s31): complete pinned `libnet80211[ieee80211_crypto.o]::ieee80211_crypto_encap` and `[ieee80211_crypto_ccmp.o]::ccmp_encap`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedManagementFrame<'a> {
    pub subtype: StaManagementSubtype,
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub sequence_number: SequenceNumber,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub body: &'a [u8],
}

impl StaProtectedManagementFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid.get())?;
        let body_start = MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN;
        let required =
            body_start
                .checked_add(self.body.len())
                .ok_or(StationFrameError::OutputTooSmall {
                    required: usize::MAX,
                })?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            self.subtype.frame_control() | PROTECTED_FRAME,
            self.bssid.get(),
            self.source.get(),
            self.bssid.get(),
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..body_start].copy_from_slice(&self.ccmp_header);
        frame[body_start..].copy_from_slice(self.body);
        Ok(required)
    }
}

/// SAE authentication algorithm number.
pub const SAE_AUTHENTICATION_ALGORITHM: u16 = 3;
/// SAE Commit authentication transaction.
pub const SAE_COMMIT_TRANSACTION: u16 = 1;
/// SAE Confirm authentication transaction.
pub const SAE_CONFIRM_TRANSACTION: u16 = 2;

/// One SAE Authentication frame this station sends: a Commit or a Confirm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeAuthenticationFrame<'a> {
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub sequence_number: SequenceNumber,
    pub transaction: u16,
    pub status_code: u16,
    pub body: &'a [u8],
}

impl SaeAuthenticationFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid.get())?;
        let body_start = MANAGEMENT_HEADER_LEN + 6;
        let required =
            body_start
                .checked_add(self.body.len())
                .ok_or(StationFrameError::OutputTooSmall {
                    required: usize::MAX,
                })?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            OPEN_AUTHENTICATION_FRAME_CONTROL,
            self.bssid.get(),
            self.source.get(),
            self.bssid.get(),
            self.sequence_number,
        );
        frame[24..26].copy_from_slice(&SAE_AUTHENTICATION_ALGORITHM.to_le_bytes());
        frame[26..28].copy_from_slice(&self.transaction.to_le_bytes());
        frame[28..30].copy_from_slice(&self.status_code.to_le_bytes());
        frame[body_start..].copy_from_slice(self.body);
        Ok(required)
    }
}

/// One SAE Authentication frame the selected access point sent this station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeAuthentication<'a> {
    pub transaction: u16,
    pub status_code: u16,
    pub body: &'a [u8],
}

/// Parse an SAE Authentication frame addressed to this station and BSSID;
/// `None` for any other frame.
pub fn parse_sae_authentication(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<SaeAuthentication<'_>> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 6
        || read_u16(frame, 0)? & 0x00fc != OPEN_AUTHENTICATION_FRAME_CONTROL
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
        || read_u16(frame, 24)? != SAE_AUTHENTICATION_ALGORITHM
    {
        return None;
    }
    Some(SaeAuthentication {
        transaction: read_u16(frame, 26)?,
        status_code: read_u16(frame, 28)?,
        body: &frame[MANAGEMENT_HEADER_LEN + 6..],
    })
}

/// Parse a response addressed to this station and BSSID.
///
/// `None` means that the frame is valid input but belongs to another
/// management exchange. This lets an RX loop ignore beacons and other peers
/// without treating them as protocol errors.
pub fn parse_open_authentication_response(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<OpenAuthenticationResponse> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 6
        || read_u16(frame, 0)? & 0x00fc != OPEN_AUTHENTICATION_FRAME_CONTROL
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
        || read_u16(frame, 24)? != OPEN_SYSTEM_ALGORITHM
        || read_u16(frame, 26)? != OPEN_SYSTEM_RESPONSE_SEQUENCE
    {
        return None;
    }
    Some(OpenAuthenticationResponse {
        status_code: read_u16(frame, 28)?,
    })
}

/// Parse a Disassociation or Deauthentication sent by the selected AP.
/// Accept only unfragmented, unprotected management addressed to this station
/// or broadcast. Protected management requires the separate PMF owner and its
/// authenticated body; this parser never interprets a CCMP header as a reason.
///
/// An authentication wait must treat this as a protocol transition rather
/// than an absent response. In particular, an AP may reject the first Open
/// Authentication after a rapid station reset by first clearing its previous
/// relationship with a Deauthentication frame.
///
/// SOURCE(esp32s31): complete
/// `libnet80211[ieee80211_sta.o]::sta_recv_mgmt`: branches `.L731`
/// (Disassociation) and `.L737` (Deauthentication) read the reason code at
/// management-body offset zero (`frame + 24`) and immediately call
/// `ieee80211_sta_new_state(g_ic, 0, (reason << 8) | subtype)`.
pub fn parse_sta_disconnect(
    frame: &[u8],
    local: OwnAddress,
    bssid: Bssid,
) -> Option<StaDisconnect> {
    let header = crate::frame::DecodedHeader::decode(frame)?;
    let control = header.control();
    if control.is_data()
        || control.ds() != 0
        || control.protected()
        || control.more_fragments()
        || header.sequence_control() & 15 != 0
        || (header.receiver() != local.get() && header.receiver() != [0xff; 6])
        || header.transmitter() != bssid.get()
        || header.address3() != bssid.get()
    {
        return None;
    }

    let kind = match control.get() & 0x00fc {
        DISASSOCIATION_FRAME_CONTROL => StaDisconnectKind::Disassociation,
        DEAUTHENTICATION_FRAME_CONTROL => StaDisconnectKind::Deauthentication,
        _ => return None,
    };
    Some(StaDisconnect {
        kind,
        reason_code: read_u16(header.body(), 0)?,
    })
}
