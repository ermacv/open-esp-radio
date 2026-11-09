//! The access point's entry points on the S31 port: its admission, its
//! commands and its issuer, beside the station's, over the one driver, its
//! queues and its sink.
//!
//! Its frames go to the same queues as the station's and publish on the
//! access point's interface (the role their preparation carries). Its
//! configuration withdraws its own queued attempts and waits for its own
//! published ones; its key removals wait for no attempt of their key; its
//! group key mutations share the station's receive cut, one mutation at a
//! time. None of them holds a station command, and a held station command
//! holds none of them, apart from that one receive cut and the one removal
//! barrier the two roles share.
//!
//! A beacon is an access-point attempt that carries the beacon permission
//! and its deadline: every terminal of it returns the permission, and one
//! still queued at its deadline ends `Expired`.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCapabilities, AccessPointCommand, AccessPointCommands, AccessPointPairwiseIssue,
    AccessPointPort, AccessPointProgress, AccessPointReceive, AccessPointRefusal,
    AccessPointServices, AccessPointSubmission, AccessPointTypes, BeaconParts, BeaconSubmission,
    BeaconTimestamp, ConfigurePermission, SupportedTsfControl, TsfControl,
};

/// An access-point command of the port's types.
pub(super) type ApCmd<'id, Id, D, const B: usize, O> =
    AccessPointCommand<'id, Types<Id, D, B, O>, Esp32s31Caps<B>>;

/// How far the port took an access-point command.
type ApProgress<'id, Id, D, const B: usize, O> =
    AccessPointProgress<'id, Types<Id, D, B, O>, Esp32s31Caps<B>>;

/// The access point's configure permission on the port.
type ApConfigure<'id, Id, D, const B: usize> =
    ConfigurePermission<'id, AccessPoint, Part<Id, 9>, <D as Device<B>>::ConfigureContext<'id, Id>>;

/// An accepted pairwise installation of the access point. Only
/// [`Esp32s31Commands`] makes one.
pub struct AccessPointPairwiseInstall<Id: SlotIdentity>(PairwiseInstall<Id>);

/// An accepted group installation of the access point. Only
/// [`Esp32s31Commands`] makes one.
pub struct AccessPointGroupInstall<Id: SlotIdentity>(GroupInstall<Id>);

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    AccessPointTypes<'id> for Types<Id, D, B, O>
{
    type AccessPointPairwiseInstall = AccessPointPairwiseInstall<Id>;
    type AccessPointGroupInstall = AccessPointGroupInstall<Id>;
}

impl<const B: usize> AccessPointCapabilities for Esp32s31Caps<B> {
    const ACCESS_POINT: AccessPointServices = AccessPointServices {
        // The caller writes the Timestamp field; the S31 publishes the MPDU
        // as written. SOURCE(esp32s31): complete pinned
        // `libnet80211[ieee80211_hostap.o]::ieee80211_hostap_send_beacon_process`
        // stores the MAC local-time counter (`hal_now`, MODEM_PWR+0x800) into
        // the frame's Timestamp in software before `ic_tx_pkt`, and no
        // function of the examined TX path (`ppTxPkt`, `ppTxProtoProc`,
        // `lmacSetTxFrame`, `hal_mac_tx_set_ppdu`, `mac_tx_set_plcp0`) sets a
        // timestamp-insertion bit or rewrites the field; the S31 TX
        // descriptor model has no such field. Whether the MAC rewrites the
        // bytes on air is not established by these sources.
        beacon_timestamp: Some(BeaconTimestamp::Caller),
        // SOURCE(esp32s31): complete pinned `libpp[hal_mac.o]::hal_mac_tsf_reset`
        // (the SoftAP restart) and `libpp[hal_tsf.o]::hal_disable_softap_tsf`,
        // the HAL's `reset_and_start_access_point_tsf` and
        // `stop_access_point_tsf`.
        tsf_control: true,
    };
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    AccessPointCommands<'id> for Esp32s31Commands<'id, Id, D, B, O>
{
    type Types = Types<Id, D, B, O>;
    type Caps = Esp32s31Caps<B>;

    fn install_access_point_pairwise(
        &mut self,
        association: &<PortKeys<Id, AccessPoint> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Id, AccessPoint> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> AccessPointPairwiseIssue<'id, Self::Types, Self::Caps> {
        let identity = <AccessPoint as Role<'id, Id>>::identity(
            association,
            &slot,
            self.serials.take(),
            self.key_live,
        );
        let issued = [association.generation(), slot.generation()];
        let installation = <AccessPoint as Role<'id, Id>>::pairwise(association, slot, peer)
            .into_owned(&self.loan);
        (
            AccessPointCommand::InstallPairwise(AccessPointPairwiseInstall(PairwiseInstall {
                identity,
                installation,
                material: key,
                issued,
            })),
            KeyIdentity {
                owned: identity,
                brand: PhantomData,
            },
        )
    }

    fn install_access_point_group(
        &mut self,
        association: &<PortKeys<Id, AccessPoint> as KeyTypes<'id>>::Association,
        permission: <PortKeys<Id, AccessPoint> as KeyTypes<'id>>::GroupPermission,
        slot: <PortKeys<Id, AccessPoint> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> AccessPointCommand<'id, Self::Types, Self::Caps> {
        AccessPointCommand::InstallGroup(AccessPointGroupInstall(GroupInstall {
            association: association.generation(),
            command: <AccessPoint as Role<'id, Id>>::group(
                association,
                slot,
                permission,
                key_id,
                key,
            )
            .into_owned(&self.loan),
        }))
    }
}

/// The group key id of an access-point group installation that the S31
/// AP key layout has no entry for ([`crate::mac::crypto::ApGroupKeyId`]).
fn unsupported_group_key_id<
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
>(
    command: &ApCmd<'id, Id, D, B, O>,
) -> bool {
    matches!(
        command,
        AccessPointCommand::InstallGroup(AccessPointGroupInstall(GroupInstall {
            command: GroupCommand::InstallAccessPoint { key_id, .. },
            ..
        })) if crate::mac::crypto::ApGroupKeyId::new(key_id.get()).is_none()
    )
}

/// The generations of the installation tokens an access-point command
/// names: its permission, association, key slot or installed key.
fn access_point_generations<
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
>(
    command: &ApCmd<'id, Id, D, B, O>,
) -> [Option<u32>; 3] {
    match command {
        AccessPointCommand::InstallPairwise(AccessPointPairwiseInstall(install)) => {
            let [association, slot] = install.issued;
            [Some(association), Some(slot), None]
        }
        AccessPointCommand::InstallGroup(AccessPointGroupInstall(install)) => {
            match &install.command {
                GroupCommand::InstallAccessPoint {
                    slot, permission, ..
                } => [
                    Some(install.association),
                    Some(slot.generation()),
                    Some(permission.generation()),
                ],
                // The issuer makes the access point's installations only.
                _ => [Some(install.association), None, None],
            }
        }
        AccessPointCommand::RemovePairwise { association, key } => [
            Some(association.generation()),
            Some(key.identity.generation()),
            None,
        ],
        AccessPointCommand::RemoveGroup { permission, key } => [
            Some(permission.generation()),
            Some(key.identity.generation()),
            None,
        ],
        AccessPointCommand::Configure { permission, .. } => {
            [Some(permission.generation()), None, None]
        }
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Port<'id, Id, D, B, O>
{
    /// Keep an access-point command with the quarantine.
    fn retain_access_point(&mut self, command: ApCmd<'id, Id, D, B, O>) {
        match self.retained_access_point {
            None => self.retained_access_point = Some(command),
            // A later command stays quarantined with the first.
            Some(_) => {
                let _quarantined = core::mem::ManuallyDrop::new(command);
            }
        }
    }

    /// Hold an access-point configuration or pairwise removal at its
    /// barrier, or refuse it past its bound. A held command has not run, so
    /// its outcome is known: what its hold began is undone (the
    /// configuration's withdrawal of the access point's attempts, the
    /// removal's barrier and its key's activity) and it comes back
    /// `Expired` with every owner.
    fn hold_access_point(
        &mut self,
        command: ApCmd<'id, Id, D, B, O>,
        now: Instant,
    ) -> Result<ApProgress<'id, Id, D, B, O>, Fault> {
        let kind = command.kind();
        let since = *self.access_point_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= COMMAND_BOUND {
            self.access_point_since = None;
            self.access_point_holding = None;
            match &command {
                AccessPointCommand::Configure { .. } => {
                    self.driver.access_point_configuring = false;
                }
                AccessPointCommand::RemovePairwise { key, .. } => {
                    let identity =
                        <PortKeys<Id, AccessPoint> as KeyTypes<'id>>::pairwise_identity(key).owned;
                    // Only its own barrier began: another key's held it
                    // before it took any step.
                    if self.driver.barrier == Some(identity) {
                        identity.activity.set(identity.serial);
                        self.driver.barrier = None;
                    }
                }
                // Group mutations expire through their own cut.
                AccessPointCommand::InstallPairwise(_)
                | AccessPointCommand::InstallGroup(_)
                | AccessPointCommand::RemoveGroup { .. } => {}
            }
            return Ok(AccessPointProgress::Refused(
                command,
                AccessPointRefusal::Expired,
            ));
        }
        self.access_point_holding = Some(kind);
        Ok(AccessPointProgress::Held(command))
    }

    /// Whether an attempt of the access point is published.
    fn access_point_on_air(&self) -> bool {
        self.driver
            .running
            .iter()
            .flatten()
            .any(|pending| pending.operation.payload().role == VifRole::AccessPoint)
    }

    /// Set what the access point receives once none of its attempts is on
    /// the air, then its TSF control: its queued attempts end withdrawn
    /// meanwhile. The station's configuration in the core's bank holds it
    /// too: one interface configures at a time.
    fn configure_access_point<S>(
        &mut self,
        permission: ApConfigure<'id, Id, D, B>,
        receive: AccessPointReceive<Esp32s31Caps<B>>,
        tsf: Option<SupportedTsfControl<Esp32s31Caps<B>>>,
        now: Instant,
        sink: &mut S,
    ) -> Result<ApProgress<'id, Id, D, B, O>, Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        self.driver.access_point_configuring = true;
        let drained = {
            let (driver, _, mut env) = self.parts(sink);
            driver.withdraw(&mut env)
        };
        if !drained
            || self.access_point_on_air()
            || core_views::configuring::<Id, D, B>(self.driver.core.view())
        {
            return self.hold_access_point(
                AccessPointCommand::Configure {
                    permission,
                    receive,
                    tsf,
                },
                now,
            );
        }
        let address = self.addresses[1].get();
        let filter = match receive {
            AccessPointReceive::Off => ReceiveFilter::NONE,
            AccessPointReceive::Bss { filter } => filter.get(),
        };
        core_views::configure::<Id, D, B>(
            self.driver.core.view(),
            CoreVif {
                role: VifRole::AccessPoint,
                address,
                bssid: Some(address),
                receive: filter,
            },
        )?;
        let stopping = self.stopping();
        {
            let (driver, device, mut env) = self.parts(sink);
            let mut steps = Sink::default();
            // No attempt of the access point is published: its barrier in
            // the core passes at once.
            let done = core_views::advance_configuration::<Id, D, B>(
                driver.core.view(),
                device.hardware()?,
                &mut steps,
            );
            driver.collect(&mut env, &mut steps, stopping)?;
            if !done {
                return Err(Fault::ConfigurationRefused);
            }
            if let Some(control) = tsf {
                let mut view = driver
                    .core
                    .view()
                    .commands()
                    .ok_or(Fault::CoreViewUnavailable(CoreOp::Configure))?;
                match control.get() {
                    TsfControl::Restart => view.restart_access_point_tsf(device.hardware()?),
                    TsfControl::Stop => view.stop_access_point_tsf(device.hardware()?),
                }
            }
            driver.access_point_configuring = false;
        }
        self.access_point_holding = None;
        self.access_point_since = None;
        let loan = self.loan;
        sink.terminal(
            brand::OwnedTerminal::Configured(brand::OwnedConfigured::AccessPoint(
                permission.into_owned(&loan),
            ))
            .lend(&loan),
        );
        Ok(AccessPointProgress::Done)
    }

    /// Run one access-point command of a running port.
    fn run_access_point<S>(
        &mut self,
        command: ApCmd<'id, Id, D, B, O>,
        now: Instant,
        sink: &mut S,
    ) -> Result<ApProgress<'id, Id, D, B, O>, Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        match command {
            AccessPointCommand::Configure {
                permission,
                receive,
                tsf,
            } => self.configure_access_point(permission, receive, tsf, now, sink),
            AccessPointCommand::InstallPairwise(AccessPointPairwiseInstall(PairwiseInstall {
                identity,
                installation,
                material,
                issued: _,
            })) => {
                self.driver
                    .install(self.device, identity, installation, &material)?;
                identity.activity.set(identity.serial);
                sink.terminal(
                    brand::OwnedTerminal::PairwiseKeyInstalled(brand::OwnedInstalledPairwise::new(
                        identity.role,
                        identity,
                    ))
                    .lend(&self.loan),
                );
                Ok(AccessPointProgress::Done)
            }
            AccessPointCommand::RemovePairwise { association, key } => {
                let identity =
                    <PortKeys<Id, AccessPoint> as KeyTypes<'id>>::pairwise_identity(&key).owned;
                // The removal barrier is one for both roles: another key's
                // removal holds this one.
                if self
                    .driver
                    .barrier
                    .is_some_and(|barrier| barrier != identity)
                {
                    return self.hold_access_point(
                        AccessPointCommand::RemovePairwise { association, key },
                        now,
                    );
                }
                identity.activity.set(0);
                self.driver.barrier = Some(identity);
                let drained = {
                    let (driver, _, mut env) = self.parts(sink);
                    driver.withdraw(&mut env)
                };
                let in_use = self
                    .driver
                    .running
                    .iter()
                    .flatten()
                    .any(|running| running.operation.payload().key == Some(identity));
                if !drained || in_use {
                    return self.hold_access_point(
                        AccessPointCommand::RemovePairwise { association, key },
                        now,
                    );
                }
                let removed = match self.driver.remove(self.device, identity) {
                    Ok(removed) => removed,
                    Err(fault) => {
                        self.retain_access_point(AccessPointCommand::RemovePairwise {
                            association,
                            key,
                        });
                        return Err(fault);
                    }
                };
                let removal =
                    <AccessPoint as Role<'id, Id>>::removal(association).into_owned(&self.loan);
                let terminal = self.driver.removed_terminal(removal, removed)?;
                self.driver.barrier = None;
                self.access_point_holding = None;
                self.access_point_since = None;
                sink.terminal(brand::OwnedTerminal::KeyRemoved(terminal).lend(&self.loan));
                Ok(AccessPointProgress::Done)
            }
            group @ (AccessPointCommand::InstallGroup(_)
            | AccessPointCommand::RemoveGroup { .. }) => {
                let kind = group.kind();
                let removal = match &group {
                    AccessPointCommand::RemoveGroup { key, .. } => Some(key.identity),
                    _ => None,
                };
                let step = self.group_mutation(
                    group,
                    Mutator::AccessPoint(kind),
                    removal,
                    removal,
                    now,
                    sink,
                    |command, loan| match command {
                        AccessPointCommand::InstallGroup(AccessPointGroupInstall(
                            GroupInstall { command, .. },
                        )) => Ok(command),
                        AccessPointCommand::RemoveGroup { permission, key } => {
                            Ok(GroupCommand::Remove {
                                identity: <AccessPoint as Role<'id, Id>>::group_identity(key),
                                removal: <AccessPoint as Role<'id, Id>>::remove_group(permission)
                                    .into_owned(loan),
                            })
                        }
                        other => Err(other),
                    },
                )?;
                match step {
                    GroupStep::Done => Ok(AccessPointProgress::Done),
                    GroupStep::Held(command) => Ok(AccessPointProgress::Held(command)),
                    GroupStep::Expired(command) => Ok(AccessPointProgress::Refused(
                        command,
                        AccessPointRefusal::Expired,
                    )),
                    GroupStep::Failed(command, fault) => {
                        self.retain_access_point(command);
                        Err(self.record(fault))
                    }
                }
            }
        }
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    AccessPointPort<'id> for Esp32s31Port<'id, Id, D, B, O>
{
    fn access_point_address(&self) -> IndividualAddress {
        let [_, access_point] = self.addresses;
        access_point
    }

    fn admit_access_point<'k, 'p>(
        &self,
        attempt: AccessPointSubmission<'k, 'p, 'id, Self>,
    ) -> Self::Admitted {
        attempt.into_parts(&self.loan)
    }

    fn admit_beacon<'k, 'p>(&self, beacon: BeaconSubmission<'k, 'p, 'id, Self>) -> Self::Admitted {
        let BeaconParts {
            mut attempt,
            permission,
            stamp: _,
            deadline,
        } = beacon.into_parts(&self.loan);
        attempt.execution.beacon = Some((permission.into_owned(&self.loan), deadline));
        attempt
    }

    fn access_point_command(
        &mut self,
        command: ApCmd<'id, Id, D, B, O>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<ApProgress<'id, Id, D, B, O>, Self::Fault> {
        if let Err(fault) = self.check() {
            self.retain_access_point(command);
            return Err(fault);
        }
        self.now = now;
        // A token of an earlier installation is refused for good, before
        // the radio's own state is asked.
        let installation = self.serials.generation;
        if let Some(issued) = access_point_generations::<Id, D, B, O>(&command)
            .into_iter()
            .flatten()
            .find(|issued| *issued != installation)
        {
            return Ok(AccessPointProgress::Refused(
                command,
                AccessPointRefusal::StalePermission {
                    issued,
                    installation,
                },
            ));
        }
        // A group key id the AP's key layout has no entry for refuses for
        // good, before the radio's own state is asked: the receive cut has
        // not begun.
        if unsupported_group_key_id::<Id, D, B, O>(&command) {
            return Ok(AccessPointProgress::Refused(
                command,
                AccessPointRefusal::UnsupportedGroupKeyId,
            ));
        }
        if !self.running() {
            return Ok(AccessPointProgress::Refused(
                command,
                AccessPointRefusal::NotEnabled,
            ));
        }
        let progress = self.run_access_point(command, now, sink);
        let progress = progress.map_err(|fault| self.record(fault));
        self.recompute_deadline();
        progress
    }
}
