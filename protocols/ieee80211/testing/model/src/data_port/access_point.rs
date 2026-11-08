//! The model's access-point role on its one port: the same queues, key
//! table, sink and stop as the station's, entered through the access
//! point's own permissions.

use super::*;

impl<'id, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    ModelPort<'id, Id, C, X>
{
    /// Whether an access-point MPDU is published.
    fn access_point_on_air(&self) -> bool {
        self.driver.queues.iter().any(|queue| {
            queue
                .air
                .as_ref()
                .is_some_and(|publication| publication.work.role() == VifRole::AccessPoint)
        })
    }

    /// Install a pairwise key of either role: the one key table both share.
    pub(super) fn install_pairwise_key(
        &mut self,
        install: ModelPairwiseInstall<Backend<Id, C, X>>,
    ) {
        let ModelPairwiseInstall {
            spp,
            association: _,
            key,
            material,
            installed,
        } = install;
        self.air.apply_association(key.0.role, spp);
        self.air.install_material(&key, material);
        self.driver.keys.push(key);
        self.held.post(OwnedLowerMacEvent::Terminal(
            OwnedLowerMacTerminal::PairwiseKeyInstalled(installed),
        ));
    }

    /// The generations of the installation tokens `command` names: its
    /// permission, association and key slots, as their issuer recorded them.
    fn command_generations(command: &ModelAccessPointCommand<'id, Id, C, X>) -> [Option<u32>; 3] {
        match command {
            AccessPointCommand::InstallPairwise(ModelAccessPointPairwiseInstall(install)) => [
                Some(install.association),
                Some(install.key.0.generation),
                None,
            ],
            AccessPointCommand::InstallGroup(ModelAccessPointGroupInstall(ModelGroupInstall(
                install,
            ))) => match install {
                GroupInstallationCommand::AccessPoint(pending) => [
                    Some(pending.association),
                    Some(pending.slot.generation()),
                    Some(pending.permission.generation()),
                ],
                GroupInstallationCommand::Station(pending) => [
                    Some(pending.association),
                    Some(pending.slot.generation()),
                    Some(pending.permission.generation()),
                ],
            },
            AccessPointCommand::RemovePairwise { association, key } => [
                Some(association.owner.generation()),
                Some(key.slot.generation()),
                None,
            ],
            AccessPointCommand::RemoveGroup { permission, key } => [
                Some(permission.generation()),
                Some(key.slot.generation()),
                None,
            ],
            AccessPointCommand::Configure { permission, .. } => {
                [Some(permission.generation()), None, None]
            }
        }
    }

    /// The first token of an earlier installation `command` names.
    fn stale_command(&self, command: &ModelAccessPointCommand<'id, Id, C, X>) -> Option<u32> {
        let generations = Self::command_generations(command);
        let named: Vec<u32> = generations.into_iter().flatten().collect();
        first_stale(self.installation.generation(), &named)
    }

    /// Run one access-point command.
    fn run_access_point(
        &mut self,
        command: ModelAccessPointCommand<'id, Id, C, X>,
        now: Instant,
    ) -> ModelAccessPointProgress<'id, Id, C, X> {
        let group_held = self.air.group_held.get() && !self.held.draining.get();
        match command {
            AccessPointCommand::Configure {
                permission,
                receive,
                tsf,
            } => {
                // Its queued attempts end withdrawn; the station's stay.
                self.driver
                    .publish_before(&self.held, |role| role == VifRole::AccessPoint);
                if self.access_point_on_air() {
                    return AccessPointProgress::Held(AccessPointCommand::Configure {
                        permission,
                        receive,
                        tsf,
                    });
                }
                let mut interface = self.air.access_point.get();
                interface.receive = match receive {
                    AccessPointReceive::Off => None,
                    AccessPointReceive::Bss { filter } => Some(filter.get()),
                };
                if let Some(control) = tsf {
                    interface.tsf = match control.get() {
                        TsfControl::Restart => ModelAccessPointTsf::Running { since: now },
                        TsfControl::Stop => ModelAccessPointTsf::Stopped,
                    };
                }
                self.air.access_point.set(interface);
                self.held.post(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::Configured(OwnedConfigured::AccessPoint(
                        permission.into_owned(&self.loan),
                    )),
                ));
                AccessPointProgress::Done
            }
            AccessPointCommand::InstallPairwise(ModelAccessPointPairwiseInstall(install)) => {
                self.install_pairwise_key(install);
                AccessPointProgress::Done
            }
            AccessPointCommand::InstallGroup(install) if group_held => {
                AccessPointProgress::Held(AccessPointCommand::InstallGroup(install))
            }
            AccessPointCommand::InstallGroup(ModelAccessPointGroupInstall(ModelGroupInstall(
                install,
            ))) => {
                self.driver.group_change(
                    &self.held,
                    now.as_micros(),
                    GroupChange::Install(install),
                );
                AccessPointProgress::Done
            }
            command @ AccessPointCommand::RemoveGroup { .. } if group_held => {
                AccessPointProgress::Held(command)
            }
            AccessPointCommand::RemoveGroup { permission, key } => {
                let identity = key.identity.clone();
                self.driver.group_change(
                    &self.held,
                    now.as_micros(),
                    GroupChange::Remove {
                        key: identity.clone(),
                        removed: OwnedRemoved::retain(
                            Removed::AccessPoint(RemovedKey::Group {
                                slot: key.slot,
                                permission,
                            }),
                            &self.loan,
                        ),
                    },
                );
                self.driver.publish(&self.held);
                self.driver.finish_removals(&self.held);
                match self
                    .unremove(&identity)
                    .map(|removed| removed.lend(&self.loan))
                {
                    Some(Removed::AccessPoint(RemovedKey::Group { slot, permission })) => {
                        AccessPointProgress::Held(AccessPointCommand::RemoveGroup {
                            permission,
                            key: ModelGroupKey { identity, slot },
                        })
                    }
                    Some(other) => {
                        self.restore_removal(identity, OwnedRemoved::retain(other, &self.loan));
                        AccessPointProgress::Done
                    }
                    None => AccessPointProgress::Done,
                }
            }
            AccessPointCommand::RemovePairwise { association, key } => {
                let ModelPairwiseKey { identity, slot } = key;
                identity.0.active.set(false);
                self.driver.removing.push(Removal {
                    key: identity.clone(),
                    removed: OwnedRemoved::retain(
                        Removed::AccessPoint(RemovedKey::Pairwise { slot, association }),
                        &self.loan,
                    ),
                    group_admitted_at: None,
                });
                self.driver.publish(&self.held);
                self.driver.finish_removals(&self.held);
                match self
                    .unremove(&identity)
                    .map(|removed| removed.lend(&self.loan))
                {
                    Some(Removed::AccessPoint(RemovedKey::Pairwise { slot, association })) => {
                        AccessPointProgress::Held(AccessPointCommand::RemovePairwise {
                            association,
                            key: ModelPairwiseKey { identity, slot },
                        })
                    }
                    Some(other) => {
                        self.restore_removal(identity, OwnedRemoved::retain(other, &self.loan));
                        AccessPointProgress::Done
                    }
                    None => AccessPointProgress::Done,
                }
            }
        }
    }
}

impl<'id, Id: SlotIdentity + 'id, C: AccessPointCapabilities + 'id, X: ModelContext + 'id>
    AccessPointPort<'id> for ModelPort<'id, Id, C, X>
{
    fn access_point_address(&self) -> IndividualAddress {
        let [_, access_point] = self.addresses;
        access_point
    }

    fn admit_access_point<'k, 'p>(
        &self,
        attempt: AccessPointSubmission<'k, 'p, 'id, Self>,
    ) -> ModelAdmitted<Id, C, X> {
        attempt.into_parts(&self.loan)
    }

    fn admit_beacon<'k, 'p>(
        &self,
        beacon: BeaconSubmission<'k, 'p, 'id, Self>,
    ) -> ModelAdmitted<Id, C, X> {
        let BeaconParts {
            mut attempt,
            permission,
            stamp: _,
        } = beacon.into_parts(&self.loan);
        attempt.execution.beacon = Some(permission.into_owned(&self.loan));
        attempt
    }

    fn access_point_command(
        &mut self,
        command: ModelAccessPointCommand<'id, Id, C, X>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<ModelAccessPointProgress<'id, Id, C, X>, ModelPortFault> {
        if let Err(fault) = self.check(ModelPortCall::Command) {
            self.retained_access_point.push(command);
            return Err(fault);
        }
        // A token of an earlier installation is refused for good, before
        // the radio's own state is asked.
        if let Some(issued) = self.stale_command(&command) {
            return Ok(AccessPointProgress::Refused(
                command,
                AccessPointRefusal::StalePermission {
                    issued,
                    installation: self.installation.generation(),
                },
            ));
        }
        if self.radio != Radio::Running {
            return Ok(AccessPointProgress::Refused(
                command,
                AccessPointRefusal::NotEnabled,
            ));
        }
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        let kind = command.kind();
        let progress = self.run_access_point(command, now);
        self.drain(sink);
        if let Err(fault) = self.check(ModelPortCall::Command) {
            if let AccessPointProgress::Held(command) | AccessPointProgress::Refused(command, _) =
                progress
            {
                self.retained_access_point.push(command);
            }
            return Err(fault);
        }
        let progress = match progress {
            AccessPointProgress::Held(command) => {
                let since = match self.access_point_held_since {
                    Some((held, since)) if held == kind => since,
                    _ => now,
                };
                if now.saturating_duration_since(since) >= MODEL_HELD_COMMAND_BOUND {
                    self.retained_access_point.push(command);
                    return Err(
                        self.fail(ModelPortFault::AccessPointCommandStalled { kind, since })
                    );
                }
                self.access_point_held_since = Some((kind, since));
                AccessPointProgress::Held(command)
            }
            done => {
                self.access_point_held_since = None;
                done
            }
        };
        self.refresh_deadline(now);
        Ok(progress)
    }
}
