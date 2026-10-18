//! Single-executor evidence and UI routing, compiled unchanged by host tests.
use core::cell::Cell;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_bluetooth_gatt_trouble::security::{
    comparison::{Challenge, NumericComparison},
    gatt::Observation,
};
use oer_hil_protocol::base::RejectReason;
use oer_hil_protocol::bluetooth::{
    ConfirmGatt, FailGattResetRead, FailNextGattBondLoad, GattDecisionRecorded, GattResetReadGate,
    RestartGatt, SecureGattState,
};
use oer_hil_protocol::{
    bluetooth::BluetoothGattShutdown, bluetooth::BluetoothNumericChallenge,
    bluetooth::BluetoothSecureGattEvidence,
};

pub struct State {
    pub reset_gate: super::reset_gate::Gate,
    evidence: Cell<BluetoothSecureGattEvidence>,
    pub comparison: NumericComparison,
    pub restart: Signal<NoopRawMutex, ()>,
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        Self {
            reset_gate: super::reset_gate::Gate::new(),
            evidence: Cell::new(BluetoothSecureGattEvidence {
                epoch: 1,
                ..Default::default()
            }),
            comparison: NumericComparison::new(),
            restart: Signal::new(),
        }
    }
    pub fn stopped(&self) {
        let mut e = self.evidence.get();
        e.application_stopped = true;
        e.restarting = false;
        self.evidence.set(e);
    }
    pub fn shutdown(&self, shutdown: BluetoothGattShutdown) {
        let mut e = self.evidence.get();
        e.shutdown = Some(shutdown);
        self.evidence.set(e);
    }
    pub fn application_failure<C, S>(
        &self,
        failure: &oer_bluetooth_gatt_trouble::security::gatt::RunError<C, S>,
    ) {
        use oer_bluetooth_gatt_trouble::security::gatt::RunError;
        use oer_hil_protocol::bluetooth::BluetoothGattApplicationFailure as F;
        use trouble_host::{BleHostError, Error};
        let failure = match failure {
            RunError::Host(BleHostError::Controller(_)) => F::Controller,
            RunError::Host(BleHostError::BleHost(error)) => match error {
                Error::Hci(status) => F::HciStatus(u8::from(*status)),
                Error::Disconnected => F::Disconnected,
                Error::InvalidState => F::InvalidState,
                Error::Busy => F::Busy,
                Error::OutOfMemory
                | Error::InsufficientSpace
                | Error::NoPermits
                | Error::ConnectionLimitReached => F::Resources,
                _ => F::HostOther,
            },
            RunError::Store(_) => F::Store,
            RunError::HostNotEmpty => F::HostNotEmpty,
            RunError::InvalidBond => F::InvalidBond,
        };
        let mut e = self.evidence.get();
        e.application_failure = Some(failure);
        self.evidence.set(e);
    }
    pub fn take_bond_load_fault(&self) -> bool {
        let mut e = self.evidence.get();
        if !e.bond_load_fault_armed {
            return false;
        }
        e.bond_load_fault_armed = false;
        e.bond_load_failures = e
            .bond_load_failures
            .checked_add(1)
            .expect("fault sequence exhausted");
        self.evidence.set(e);
        true
    }
    pub fn cold(&self, old_hci_closed: bool) {
        let mut e = self.evidence.get();
        e.cold_releases = e
            .cold_releases
            .checked_add(1)
            .expect("cold sequence exhausted");
        e.old_hci_closed = old_hci_closed;
        e.traffic.advertising = false;
        e.traffic.connected = false;
        self.evidence.set(e);
    }
    pub fn restarted(&self) {
        let mut e = self.evidence.get();
        e.epoch = e.epoch.checked_add(1).expect("Host epoch exhausted");
        e.restarting = false;
        e.shutdown = None;
        e.application_failure = None;
        // The characteristic belongs to a fresh application, unlike the bond.
        e.traffic.value = 0;
        self.evidence.set(e);
    }
    /// The evidence as a reply, stamped with the reset reader gate's phase.
    fn reply(&self, mut evidence: BluetoothSecureGattEvidence) -> SecureGattState {
        evidence.reset_read_gate = self.reset_gate.phase();
        SecureGattState(evidence)
    }

    /// Fail the reached shutdown reader of epoch `request.epoch`.
    pub fn fail_reset_read(
        &self,
        request: FailGattResetRead,
    ) -> Result<SecureGattState, RejectReason> {
        let e = self.evidence.get();
        if request.epoch == e.epoch
            && !e.application_stopped
            && e.restarting
            && self.reset_gate.fail_read()
        {
            Ok(self.reply(self.evidence.get()))
        } else {
            Err(RejectReason::InvalidState)
        }
    }

    /// Arm the shutdown reader gate before a restart, or release it during
    /// one.
    pub fn reset_read_gate(
        &self,
        request: GattResetReadGate,
    ) -> Result<SecureGattState, RejectReason> {
        let e = self.evidence.get();
        if request.epoch != e.epoch || e.application_stopped {
            return Err(RejectReason::InvalidState);
        }
        let accepted = if request.release {
            e.restarting && self.reset_gate.release()
        } else {
            !e.restarting
                && e.traffic.advertising
                && !e.traffic.connected
                && !e.bond_load_fault_armed
                && e.bond_load_failures == 0
                && self.reset_gate.arm()
        };
        if accepted {
            Ok(self.reply(e))
        } else {
            Err(RejectReason::InvalidState)
        }
    }

    /// Fail the next real bond-store load after this connection ends.
    pub fn fail_next_bond_load(
        &self,
        request: FailNextGattBondLoad,
    ) -> Result<SecureGattState, RejectReason> {
        let mut e = self.evidence.get();
        if request.epoch != e.epoch
            || !e.traffic.connected
            || e.bonds_stored != 1
            || e.application_stopped
            || e.restarting
            || e.bond_load_fault_armed
            || matches!(
                self.reset_gate.phase(),
                oer_hil_protocol::bluetooth::BluetoothGattResetReadGate::Armed
            )
            || e.bond_load_failures != 0
        {
            return Err(RejectReason::InvalidState);
        }
        e.bond_load_fault_armed = true;
        self.evidence.set(e);
        Ok(self.reply(e))
    }

    /// Restart the application's Controller epoch, keeping the RAM bonds.
    pub fn restart_application(
        &self,
        request: RestartGatt,
    ) -> Result<SecureGattState, RejectReason> {
        let mut e = self.evidence.get();
        if request.epoch != e.epoch
            || e.application_stopped
            || e.restarting
            || e.bond_load_fault_armed
        {
            return Err(RejectReason::InvalidState);
        }
        e.restarting = true;
        self.evidence.set(e);
        self.restart.signal(());
        Ok(self.reply(e))
    }

    /// The evidence, with the numeric comparison awaiting a decision.
    pub fn snapshot(&self) -> SecureGattState {
        let mut value = self.evidence.get();
        value.comparison = self
            .comparison
            .pending()
            .map(|c| BluetoothNumericChallenge {
                id: c.id,
                number: c.number,
            });
        self.reply(value)
    }

    /// Queue the user's decision on the pending numeric comparison.
    pub fn confirm(&self, request: ConfirmGatt) -> Result<GattDecisionRecorded, RejectReason> {
        let e = self.evidence.get();
        if e.application_stopped || e.restarting {
            return Err(RejectReason::InvalidState);
        }
        let decision = request.0;
        self.comparison
            .respond(
                Challenge {
                    id: decision.challenge.id,
                    number: decision.challenge.number,
                },
                decision.accept,
            )
            .map(|()| GattDecisionRecorded(decision))
            .map_err(|_| RejectReason::InvalidState)
    }
    pub fn observe(&self, event: Observation) {
        let mut e = self.evidence.get();
        let inc = |n: &mut u32| *n = n.checked_add(1).expect("secure GATT evidence exhausted");
        match event {
            Observation::Ready(address) => e.traffic.address = Some(address),
            Observation::Advertising => {
                e.traffic.advertising = true;
                inc(&mut e.traffic.advertising_starts);
            }
            Observation::Connected => {
                e.traffic.advertising = false;
                e.traffic.connected = true;
                inc(&mut e.traffic.connections);
            }
            Observation::Disconnected(reason) => {
                e.traffic.connected = false;
                e.traffic.last_disconnect_reason = Some(reason);
                inc(&mut e.traffic.disconnections);
            }
            Observation::Read(value) => {
                e.traffic.value = value;
                inc(&mut e.traffic.reads);
            }
            Observation::Written(value) => {
                e.traffic.value = value;
                inc(&mut e.traffic.writes);
            }
            Observation::Comparison(_) => inc(&mut e.comparisons),
            Observation::ComparisonAccepted => inc(&mut e.accepted),
            Observation::ComparisonRejected => inc(&mut e.declined),
            Observation::BondStored => inc(&mut e.bonds_stored),
            Observation::BondResumed => inc(&mut e.bonds_resumed),
            Observation::PairingFailed => inc(&mut e.pairing_failures),
            Observation::Rejected => inc(&mut e.rejected),
            Observation::NotificationQueued(_) => inc(&mut e.notifications_queued),
        }
        self.evidence.set(e);
    }
}
