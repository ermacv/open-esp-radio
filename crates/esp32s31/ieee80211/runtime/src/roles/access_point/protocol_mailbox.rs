//! Typed protocol-to-radio actions for the AP RX ownership split.

use oer_ieee80211_mac::sequence::SequenceNumber;

/// Hardware actions inferred by the protocol consumer but executed only by
/// the radio/PAC owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointHardwareAction {
    ResetRxBlockAckWindow {
        hardware_index: u8,
        tid: u8,
        starting_sequence: SequenceNumber,
        window: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointProtocolAction {
    Hardware(AccessPointHardwareAction),
}

/// Bounded value-only handoff from the protocol consumer to the radio owner.
///
/// The current AP executor owns both endpoints. The protocol half publishes
/// while it has no PAC or AP-engine capability; after that borrow ends, the
/// radio owner drains and executes the actions. Keeping the handoff explicit
/// permits moving the consumer to an independent task without changing the
/// action vocabulary.
pub struct AccessPointProtocolMailbox<const CAPACITY: usize> {
    actions: [Option<AccessPointProtocolAction>; CAPACITY],
    head: usize,
    len: usize,
}

impl<const CAPACITY: usize> AccessPointProtocolMailbox<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            actions: [None; CAPACITY],
            head: 0,
            len: 0,
        }
    }

    pub fn publisher(&mut self) -> AccessPointProtocolPublisher<'_, CAPACITY> {
        AccessPointProtocolPublisher { mailbox: self }
    }

    pub fn receiver(&mut self) -> AccessPointProtocolReceiver<'_, CAPACITY> {
        AccessPointProtocolReceiver { mailbox: self }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn remaining_capacity(&self) -> usize {
        CAPACITY.saturating_sub(self.len)
    }

    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }

    fn push(&mut self, action: AccessPointProtocolAction) -> Result<(), AccessPointProtocolAction> {
        if self.len == CAPACITY || CAPACITY == 0 {
            return Err(action);
        }
        let tail = (self.head + self.len) % CAPACITY;
        self.actions[tail] = Some(action);
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<AccessPointProtocolAction> {
        if self.len == 0 {
            return None;
        }
        let action = self.actions[self.head].take();
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        action
    }
}

impl<const CAPACITY: usize> Default for AccessPointProtocolMailbox<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Protocol-side capability: it may publish values, but cannot execute them.
pub struct AccessPointProtocolPublisher<'mailbox, const CAPACITY: usize> {
    mailbox: &'mailbox mut AccessPointProtocolMailbox<CAPACITY>,
}

impl<const CAPACITY: usize> AccessPointProtocolPublisher<'_, CAPACITY> {
    pub fn try_publish(
        &mut self,
        action: AccessPointProtocolAction,
    ) -> Result<(), AccessPointProtocolAction> {
        self.mailbox.push(action)
    }
}

/// Radio-side capability: it may receive values, but cannot create protocol
/// conclusions or borrow their frame storage.
pub struct AccessPointProtocolReceiver<'mailbox, const CAPACITY: usize> {
    mailbox: &'mailbox mut AccessPointProtocolMailbox<CAPACITY>,
}

impl<const CAPACITY: usize> AccessPointProtocolReceiver<'_, CAPACITY> {
    pub fn try_receive(&mut self) -> Option<AccessPointProtocolAction> {
        self.mailbox.pop()
    }

    pub const fn len(&self) -> usize {
        self.mailbox.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.mailbox.is_empty()
    }
}

#[cfg(test)]
mod tests;
