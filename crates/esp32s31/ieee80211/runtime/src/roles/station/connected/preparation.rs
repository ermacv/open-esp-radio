//! Owner-preserving preparation of a successful join for connected service.
//!
//! The associated peer, selected VIF and connected policy must be validated
//! before runtime, network or key owners are split by a composition root.  A
//! failed policy check returns the complete original handoff so callers cannot
//! accidentally turn configuration failure into a partially consumed epoch.

use crate::roles::station::connected::port::{
    ConnectedStaConfigError, ConnectedStaPlan, ConnectedStaPort,
};

use oer_esp32s31_ieee80211_sta::attempt::{StaAttemptSecurity, StaInstalledSecurity};

use oer_ieee80211_runtime::station_network::{
    StationNetworkLink, StationNetworkResources, start_station_network,
};

use super::{ConnectedServiceParts, ConnectedServiceResources};

/// Validated connected plan paired with every owner returned by join.
pub struct PreparedConnectedService<'security, R, E, N> {
    runtime: R,
    epoch: E,
    network: N,
    plan: ConnectedStaPlan,
    installed_security: StaInstalledSecurity,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, E, N> PreparedConnectedService<'security, R, E, N> {
    pub const fn plan(&self) -> &ConnectedStaPlan {
        &self.plan
    }

    pub const fn epoch(&self) -> &E {
        &self.epoch
    }

    pub fn into_parts(self) -> PreparedConnectedServiceParts<'security, R, E, N> {
        PreparedConnectedServiceParts {
            runtime: self.runtime,
            epoch: self.epoch,
            network: self.network,
            plan: self.plan,
            installed_security: self.installed_security,
            security: self.security,
        }
    }
}

/// Validated service after its persistent network owner entered the connected
/// link state.
pub struct StartedNetworkConnection<'security, R, E, S, N, T> {
    runtime: R,
    epoch: E,
    stack: S,
    network: N,
    initial_network_task: Option<T>,
    plan: ConnectedStaPlan,
    installed_security: StaInstalledSecurity,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, E, S, N, T> StartedNetworkConnection<'security, R, E, S, N, T> {
    /// Borrow the still-coherent station runtime before the connected graph is
    /// decomposed for driver assembly. Platform activation failures can then
    /// return this complete owner instead of rebuilding it from loose fields.
    pub fn runtime_mut(&mut self) -> &mut R {
        &mut self.runtime
    }

    /// Exact agreement proof retained before a paired composition splits the
    /// connected owner graph. A mismatch must quarantine this whole value;
    /// it must not be repaired by treating a key-bearing attempt as Open.
    pub const fn security_modes(
        &self,
    ) -> (
        oer_ieee80211_mac::security::LinkProtection,
        oer_ieee80211_mac::security::LinkProtection,
    ) {
        (
            self.installed_security.mode(),
            self.security.policy().link_protection(),
        )
    }

    pub fn into_parts(self) -> ConnectedNetworkStartedParts<'security, R, E, S, N, T> {
        ConnectedNetworkStartedParts {
            runtime: self.runtime,
            epoch: self.epoch,
            stack: self.stack,
            network: self.network,
            initial_network_task: self.initial_network_task,
            plan: self.plan,
            installed_security: self.installed_security,
            security: self.security,
        }
    }

    /// Borrow the runtime and hardware frontier together for the final
    /// pre-activation ownership transaction.
    pub fn runtime_and_epoch_mut(&mut self) -> (&mut R, &mut E) {
        (&mut self.runtime, &mut self.epoch)
    }
}

/// Named network-started decomposition used by the concrete driver assembler.
pub struct ConnectedNetworkStartedParts<'security, R, E, S, N, T> {
    pub runtime: R,
    pub epoch: E,
    pub stack: S,
    pub network: N,
    pub initial_network_task: Option<T>,
    pub plan: ConnectedStaPlan,
    pub installed_security: StaInstalledSecurity,
    pub security: StaAttemptSecurity<'security>,
}

impl<'security, R, E, D, N, S>
    PreparedConnectedService<'security, R, E, StationNetworkResources<D, N, S>>
where
    N: StationNetworkLink,
{
    /// Start the IP stack exactly once and publish link-up for every later
    /// association while the complete radio/security handoff remains owned.
    ///
    /// The initializer may borrow runtime resource bindings (for example one
    /// static stack arena) but cannot consume or replace the runtime owner.
    pub fn start_network<T>(
        mut self,
        start: impl FnOnce(&mut R, D, &ConnectedStaPlan) -> (S, T),
    ) -> StartedNetworkConnection<'security, R, E, S, N, T> {
        let network = start_station_network(self.network, |device| {
            start(&mut self.runtime, device, &self.plan)
        });
        let (stack, network, initial_network_task) = network.into_parts();
        StartedNetworkConnection {
            runtime: self.runtime,
            epoch: self.epoch,
            stack,
            network,
            initial_network_task,
            plan: self.plan,
            installed_security: self.installed_security,
            security: self.security,
        }
    }
}

/// Named decomposition after plan validation has succeeded.
pub struct PreparedConnectedServiceParts<'security, R, E, N> {
    pub runtime: R,
    pub epoch: E,
    pub network: N,
    pub plan: ConnectedStaPlan,
    pub installed_security: StaInstalledSecurity,
    pub security: StaAttemptSecurity<'security>,
}

/// Configuration failure retaining the complete join handoff.
pub struct ConnectedServicePrepareFailure<'security, R, E, N> {
    pub error: ConnectedStaConfigError,
    resources: ConnectedServiceResources<'security, R, E, N>,
}

impl<'security, R, E, N> ConnectedServicePrepareFailure<'security, R, E, N> {
    pub fn into_resources(self) -> ConnectedServiceResources<'security, R, E, N> {
        self.resources
    }
}

/// Validate the selected VIF, associated peer and connected policy before any
/// hardware, network, key or reusable-storage owner moves independently.
#[allow(clippy::result_large_err)]
pub fn prepare_esp32s31_connected_service<
    const AGGREGATE_SLOTS: usize,
    const RX_REORDER_SLOTS: usize,
    R,
    E,
    N,
>(
    resources: ConnectedServiceResources<'_, R, E, N>,
) -> Result<PreparedConnectedService<'_, R, E, N>, ConnectedServicePrepareFailure<'_, R, E, N>> {
    let ConnectedServiceParts {
        runtime,
        epoch,
        network,
        interface,
        config,
        peer,
        installed_security,
        security,
    } = resources.into_parts();
    let installed_mode = installed_security.mode();
    let material_mode = security.policy().link_protection();
    if installed_mode != material_mode {
        return Err(ConnectedServicePrepareFailure {
            error: ConnectedStaConfigError::SecurityModeMismatch {
                installed: installed_mode,
                material: material_mode,
            },
            resources: ConnectedServiceResources::new(
                runtime,
                epoch,
                network,
                interface,
                config,
                peer,
                installed_security,
                security,
            ),
        });
    }
    match ConnectedStaPort::prepare_for_interface_with_storage_and_security::<
        AGGREGATE_SLOTS,
        RX_REORDER_SLOTS,
    >(peer, config, interface, installed_mode)
    {
        Ok(plan) => Ok(PreparedConnectedService {
            runtime,
            epoch,
            network,
            plan,
            installed_security,
            security,
        }),
        Err(failure) => Err(ConnectedServicePrepareFailure {
            error: failure.error,
            resources: ConnectedServiceResources::new(
                runtime,
                epoch,
                network,
                interface,
                config,
                failure.peer,
                installed_security,
                security,
            ),
        }),
    }
}
