//! MAC, RX and IRQ quiescence before Wi-Fi leaves the shared radio.
use super::*;
use oer_esp32s31_ieee80211_runtime::datapath::stop::{StopError, stop_mac};
use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

#[derive(Clone, Copy, Debug)]
pub(super) enum Reason {
    TxNotIdle,
    RxOwner,
    Mac(StopError),
    Rx(oer_esp32s31_ieee80211_mac::rx::RxRingError),
    Interrupt,
    InterruptOwner,
}

impl core::fmt::Display for Reason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TxNotIdle => f.write_str("ordinary TX owner is not idle"),
            Self::RxOwner => f.write_str("unexpected physical RX owner"),
            Self::Mac(error) => write!(f, "MAC stop: {error:?}"),
            Self::Rx(error) => write!(f, "RX stop: {error:?}"),
            Self::Interrupt => f.write_str("IRQ quiesce failed"),
            Self::InterruptOwner => f.write_str("inactive IRQ owner unavailable"),
        }
    }
}

type Resources = (
    ProductionWifiPhysicalResources,
    ProductionStationRoleResources,
    ProductionAccessPointResources,
    ProductionMonitorResources,
);

pub(super) struct Failure {
    _owner: ProductionWifiOwner,
    _resources: Resources,
    reason: Reason,
}
impl Failure {
    pub(super) const fn reason(&self) -> Reason {
        self.reason
    }
    /// An unconfirmed MAC or RX stop leaves the radio ambiguous and resets;
    /// a rejected quiesce retains its unchanged owners.
    pub(super) fn enforce_disposition(&self) {
        let reason = match self.reason {
            Reason::Mac(_) | Reason::Rx(_) => Some(SharedPhyFailStop::LifecycleFailed),
            Reason::TxNotIdle | Reason::RxOwner | Reason::Interrupt | Reason::InterruptOwner => {
                None
            }
        };
        crate::lifecycle_policy::enforce(self, reason);
    }
    fn radio(owner: ProductionWifiOwner, resources: Resources, reason: Reason) -> Self {
        Self {
            _owner: owner,
            _resources: resources,
            reason,
        }
    }
}

pub(super) struct ShutdownFrontier {
    wifi: WifiStopped<EspHalWifiPlatform, WifiMembership>,
    resources: Resources,
}

impl ShutdownFrontier {
    pub(super) fn into_parts(self) -> (WifiStopped<EspHalWifiPlatform, WifiMembership>, Resources) {
        (self.wifi, self.resources)
    }
}

/// Stop the MAC, halt the live RX ring and withdraw the IRQ route, returning
/// the cold Wi-Fi owner that may leave the shared radio.
// CAPABILITY: whole-radio-active-operation-power-saving-and-shutdown-full-powered-shutdown
pub(super) async fn quiesce_for_shutdown(
    stopped: ProductionSupervisorStopped,
) -> Result<ShutdownFrontier, Failure> {
    let (wifi, mut physical, station, access_point, monitor) = stopped.into_parts();
    let tx_idle = match &physical.tx {
        ProductionOrdinaryTxResources::Uninitialized(_) => true,
        ProductionOrdinaryTxResources::Epoch(epoch) => {
            epoch.control().is_ok_and(|control| control.is_idle())
        }
    };
    if !tx_idle {
        return Err(Failure::radio(
            wifi,
            (physical, station, access_point, monitor),
            Reason::TxNotIdle,
        ));
    }
    // AggregateTxResources is the idle typestate, already returned by the role.
    if matches!(&wifi, ProductionWifiOwner::Live { .. }) && physical.rx_ring.is_none() {
        return Err(Failure::radio(
            wifi,
            (physical, station, access_point, monitor),
            Reason::RxOwner,
        ));
    }
    let wifi = match wifi {
        ProductionWifiOwner::Cold(stopped) => stopped,
        ProductionWifiOwner::Live {
            mut owner,
            mut registers,
            mut interrupts,
        } => {
            if let Err(error) = await_stack_boundary!(stop_mac(
                &mut registers,
                &EmbassyClock,
                oer_time::Duration::from_millis(100),
            )) {
                return Err(Failure::radio(
                    ProductionWifiOwner::Live {
                        owner,
                        registers,
                        interrupts,
                    },
                    (physical, station, access_point, monitor),
                    Reason::Mac(error),
                ));
            }
            if let Some(ring) = physical.rx_ring.take() {
                physical.rx_ring = Some(match ring {
                    ProductionRxRing::Halted(ring) => ProductionRxRing::Halted(ring),
                    ProductionRxRing::Live(ring) => match ring.try_stop(&mut registers) {
                        Ok(ring) => ProductionRxRing::Halted(ring),
                        Err((ring, error)) => {
                            physical.rx_ring = Some(ProductionRxRing::Live(ring));
                            return Err(Failure::radio(
                                ProductionWifiOwner::Live {
                                    owner,
                                    registers,
                                    interrupts,
                                },
                                (physical, station, access_point, monitor),
                                Reason::Rx(error),
                            ));
                        }
                    },
                });
            }
            if interrupts.is_active() {
                let platform = owner.platform_mut();
                if let Err(error) = interrupts.quiesce(platform) {
                    diagnostics_event!("open-radio: shutdown IRQ quiesce failed: {:?}", error);
                    return Err(Failure::radio(
                        ProductionWifiOwner::Live {
                            owner,
                            registers,
                            interrupts,
                        },
                        (physical, station, access_point, monitor),
                        Reason::Interrupt,
                    ));
                }
            }
            let (_, setup, _, _) = match interrupts.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(interrupts) => {
                    return Err(Failure::radio(
                        ProductionWifiOwner::Live {
                            owner,
                            registers,
                            interrupts,
                        },
                        (physical, station, access_point, monitor),
                        Reason::InterruptOwner,
                    ));
                }
            };
            owner.into_stopped(registers, setup, ()).wifi
        }
    };
    Ok(ShutdownFrontier {
        wifi,
        resources: (physical, station, access_point, monitor),
    })
}
