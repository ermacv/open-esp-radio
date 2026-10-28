//! Data Length Update negotiation state.

use oer_bluetooth_le_wire::data_length::{LeDataLength, LeDataLengths};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalRequest {
    None,
    Queued,
    Transmitted,
}

/// Why a length control PDU was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LeDataLengthMalformed;

/// The Data Length Update state of one connection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeDataLengthProcedure {
    local: LeDataLengths,
    remote: LeDataLengths,
    effective: LeDataLengths,
    request: LocalRequest,
    changed: bool,
}

impl LeDataLengthProcedure {
    /// A connection that sends up to `transmit` and receives up to the
    /// supported maximum; it starts at the minimum both ways.
    pub(crate) const fn new(transmit: LeDataLength) -> Self {
        Self {
            local: LeDataLengths {
                transmit: transmit.supported(),
                receive: LeDataLength::SUPPORTED_MAXIMUM,
            },
            remote: LeDataLengths::MINIMUM,
            effective: LeDataLengths::MINIMUM,
            request: LocalRequest::None,
            changed: false,
        }
    }

    pub(crate) const fn effective(&self) -> LeDataLengths {
        self.effective
    }

    pub(crate) const fn local_request_pending(&self) -> bool {
        !matches!(self.request, LocalRequest::None)
    }

    pub(crate) const fn local_request_transmitted(&self) -> bool {
        matches!(self.request, LocalRequest::Transmitted)
    }

    /// Ask to send up to `transmit`. The procedure starts when the local
    /// length differs from what the peer last learned.
    pub(crate) fn set_transmit(&mut self, transmit: LeDataLength) {
        let transmit = transmit.supported();
        if self.local.transmit == transmit {
            return;
        }
        self.local.transmit = transmit;
        if matches!(self.request, LocalRequest::None) {
            self.request = LocalRequest::Queued;
        }
    }

    /// Start the procedure when the local length exceeds the minimum, as a
    /// new connection with a longer suggested default does.
    pub(crate) fn start_if_extended(&mut self) {
        if self.local.transmit != LeDataLength::MINIMUM
            && matches!(self.request, LocalRequest::None)
        {
            self.request = LocalRequest::Queued;
        }
    }

    /// The `LL_LENGTH_REQ` waiting for transmission.
    pub(crate) fn queued_request(&self) -> Option<[u8; 9]> {
        matches!(self.request, LocalRequest::Queued).then(|| self.pdu(0x14))
    }

    pub(crate) fn request_enqueued(&mut self) {
        if matches!(self.request, LocalRequest::Queued) {
            self.request = LocalRequest::Transmitted;
        }
    }

    /// Answer a peer's `LL_LENGTH_REQ` body with the `LL_LENGTH_RSP`.
    pub(crate) fn receive_request(
        &mut self,
        body: &[u8],
    ) -> Result<[u8; 9], LeDataLengthMalformed> {
        self.learn(body)?;
        Ok(self.pdu(0x15))
    }

    /// Complete the local procedure with the peer's `LL_LENGTH_RSP` body. A
    /// response nobody asked for is ignored.
    pub(crate) fn receive_response(&mut self, body: &[u8]) -> Result<(), LeDataLengthMalformed> {
        if !matches!(self.request, LocalRequest::Transmitted) {
            return Ok(());
        }
        self.learn(body)?;
        self.request = LocalRequest::None;
        Ok(())
    }

    /// The peer does not support the procedure, or the response timed out:
    /// the lengths stay as they are.
    pub(crate) fn abandon(&mut self) {
        if matches!(self.request, LocalRequest::Transmitted) {
            self.request = LocalRequest::None;
        }
    }

    /// The new effective lengths, once after each change.
    pub(crate) fn take_change(&mut self) -> Option<LeDataLengths> {
        core::mem::take(&mut self.changed).then_some(self.effective)
    }

    fn learn(&mut self, body: &[u8]) -> Result<(), LeDataLengthMalformed> {
        self.remote = LeDataLengths::from_wire(body).ok_or(LeDataLengthMalformed)?;
        let effective = LeDataLengths {
            transmit: self.local.transmit.min(self.remote.receive),
            receive: self.local.receive.min(self.remote.transmit),
        };
        if effective != self.effective {
            self.effective = effective;
            self.changed = true;
        }
        Ok(())
    }

    fn pdu(&self, opcode: u8) -> [u8; 9] {
        let mut pdu = [0; 9];
        pdu[0] = opcode;
        for (slot, byte) in pdu.iter_mut().skip(1).zip(self.local.wire()) {
            *slot = byte;
        }
        pdu
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
