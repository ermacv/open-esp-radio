//! Fixed initial-key or refresh-key failure probe. No application data is sent on the failed link.
use super::{Adapter, PeerAddress};
use oer_hil_protocol::bluetooth::BluetoothSecurityFailure as Failure;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub adapter: String,
    pub peer: String,
    pub failure: Failure,
    pub read_version_before_disconnect: bool,
    pub version_command_status: bool,
    pub remote_version: Option<(u8, u16, u16)>,
    /// Bounded HCI observation chronology, relative to encryption command submission.
    pub events: Vec<Observation>,
    pub initial_powered: Option<bool>,
    pub initial_soft_blocked: Option<bool>,
    pub connected: bool,
    pub command_status: bool,
    pub initial_command_status: bool,
    pub initial_encrypted: bool,
    pub acl_sent: bool,
    pub encryption_failure: Option<u8>,
    pub disconnect_requested: bool,
    pub disconnect_command_status: bool,
    pub disconnect_reason: Option<u8>,
    pub completed_after_micros: Option<u64>,
    pub restored: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Observation {
    pub after_micros: u64,
    /// Incoming H4 event bytes; this is not an over-the-air capture.
    pub packet: Vec<u8>,
}
impl Report {
    pub fn new(adapter: Adapter, peer: PeerAddress, failure: Failure) -> Self {
        Self {
            schema: 4,
            adapter: adapter.to_string(),
            peer: peer.to_string(),
            failure,
            read_version_before_disconnect: false,
            version_command_status: false,
            remote_version: None,
            events: Vec::new(),
            initial_powered: None,
            initial_soft_blocked: None,
            connected: false,
            command_status: false,
            initial_command_status: false,
            initial_encrypted: false,
            acl_sent: false,
            encryption_failure: None,
            disconnect_requested: false,
            disconnect_command_status: false,
            disconnect_reason: None,
            completed_after_micros: None,
            restored: false,
            errors: Vec::new(),
        }
    }
    pub fn passed(&self, adapter: Adapter, peer: PeerAddress, failure: Failure) -> bool {
        self.schema == 4
            && self.adapter == adapter.to_string()
            && self.peer == peer.to_string()
            && self.failure == failure
            && self.initial_powered.is_some()
            && self.initial_soft_blocked.is_some()
            && self.connected
            && self.command_status
            && self.initial_command_status == (failure == Failure::MissingRefreshKey)
            && self.initial_encrypted
                == matches!(failure, Failure::MissingRefreshKey | Failure::ActiveDataMic)
            && self.acl_sent == (failure == Failure::ActiveDataMic)
            && self.completed_after_micros.is_some()
            && self.restored
            && self.errors.is_empty()
            && if self.read_version_before_disconnect {
                self.failure == Failure::MissingKey
                    && self.version_command_status
                    && self.remote_version == Some(super::EXPECTED_REMOTE_VERSION)
            } else {
                !self.version_command_status && self.remote_version.is_none()
            }
            && match failure {
                Failure::MissingKey => {
                    self.encryption_failure == Some(6)
                        && self.disconnect_requested
                        && self.disconnect_command_status
                        && self.disconnect_reason == Some(0x16)
                }
                Failure::MissingRefreshKey => {
                    matches!(self.encryption_failure, None | Some(6))
                        && !self.disconnect_requested
                        && !self.disconnect_command_status
                        && self.disconnect_reason == Some(6)
                }
                Failure::WrongKey => {
                    matches!(self.encryption_failure, None | Some(8))
                        && !self.disconnect_requested
                        && !self.disconnect_command_status
                        && self.disconnect_reason == Some(8)
                }
                Failure::ActiveDataMic => {
                    self.encryption_failure.is_none()
                        && !self.disconnect_requested
                        && !self.disconnect_command_status
                        && self.disconnect_reason == Some(8)
                }
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_failure_and_restoration_are_required() {
        let adapter = Adapter(0);
        let peer = PeerAddress([1; 6]);
        for failure in [
            Failure::MissingKey,
            Failure::WrongKey,
            Failure::MissingRefreshKey,
            Failure::ActiveDataMic,
        ] {
            let mut r = Report::new(adapter, peer, failure);
            r.initial_powered = Some(false);
            r.initial_soft_blocked = Some(true);
            r.connected = true;
            r.command_status = true;
            r.completed_after_micros = Some(1);
            match failure {
                Failure::MissingKey => {
                    r.encryption_failure = Some(6);
                    r.disconnect_requested = true;
                    r.disconnect_command_status = true;
                    r.disconnect_reason = Some(0x16);
                }
                Failure::WrongKey => r.disconnect_reason = Some(8),
                Failure::ActiveDataMic => {
                    r.initial_encrypted = true;
                    r.acl_sent = true;
                    r.disconnect_reason = Some(8);
                }
                Failure::MissingRefreshKey => {
                    r.initial_command_status = true;
                    r.initial_encrypted = true;
                    r.disconnect_reason = Some(6);
                }
            }
            assert!(!r.passed(adapter, peer, failure));
            r.restored = true;
            assert!(r.passed(adapter, peer, failure));
            r.read_version_before_disconnect = true;
            assert!(!r.passed(adapter, peer, failure));
            r.version_command_status = true;
            r.remote_version = Some((0x0d, 0xffff, 1));
            assert_eq!(
                r.passed(adapter, peer, failure),
                failure == Failure::MissingKey
            );
            // The runner, not the helper, rejects a foreign identity.
            for foreign in [(0x0c, 0xffff, 1), (0x0d, 0x00ff, 1), (0x0d, 0xffff, 2)] {
                r.remote_version = Some(foreign);
                assert!(!r.passed(adapter, peer, failure));
            }
            r.read_version_before_disconnect = false;
            assert!(!r.passed(adapter, peer, failure));
            r.version_command_status = false;
            r.remote_version = None;
            assert!(!r.passed(Adapter(1), peer, failure));
            assert!(!r.passed(adapter, PeerAddress([2; 6]), failure));
            r.disconnect_reason = Some(0x13);
            assert!(!r.passed(adapter, peer, failure));
            r.disconnect_reason = Some(if failure == Failure::MissingKey {
                0x16
            } else {
                8
            });
            r.encryption_failure = Some(0);
            assert!(!r.passed(adapter, peer, failure));
            r.errors.push("restore failed".into());
            assert!(!r.passed(adapter, peer, failure));
        }
    }
}
