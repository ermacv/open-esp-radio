//! Generated-PAC ownership for finite receive-policy transitions.

use oer_mmio::{Index, Placement};

use oer_esp32s31_pac::wifi_mac::{WifiMacBssidPolicy, WifiMacInterfaceAddress};

use crate::ieee80211::registers::{MacInterface, MacPlacement, WifiRadioRegisters, byte};

/// The interface context of each receive queue in the cold policy: queues
/// zero to two are also the first three interface contexts; queue three has
/// none.
const COLD_RECEIVE_QUEUE_INTERFACES: [Option<MacInterface>; 4] = [
    Some(MacInterface::Station),
    Some(MacInterface::AccessPoint),
    Some(MacInterface::Context2),
    None,
];

/// The cold policy's edge on one interface's high address word.
///
/// Zero is the complete low-half replacement performed by the mode-zero cold
/// leaf; high policy bits are preserved.
fn clear_cold_interface_address<P: Placement>(
    interfaces: &mut WifiMacInterfaceAddress<P>,
    interface: MacInterface,
) {
    interfaces.modify_address_high(interface.index(), |w| {
        w.set_bytes_4_5(0);
    });
}

/// Read-only associated-STA receive-policy evidence.
///
/// Only reviewed semantic fields cross the HAL boundary. Complete policy
/// register images stay private because several bits are still
/// undocumented and no production decision consumes them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacStaReceivePolicySnapshot {
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub minimum_mpdu_start_spacing: u8,
    pub bssid_address_check_enabled: bool,
    pub interface_is_soft_ap: bool,
    pub interface_rx_policy_enabled: bool,
    pub beacon_filter_enabled: bool,
    pub broadcast_wakeup_enabled: bool,
    pub unicast_wakeup_enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacApReceivePolicySnapshot {
    pub bssid: [u8; 6],
    pub bssid_address_check_enabled: bool,
    pub interface_is_soft_ap: bool,
    pub interface_rx_policy_enabled: bool,
}

/// Closed register plan for simultaneous same-channel STA and SoftAP receive
/// contexts.
///
/// This type proves only that the two reviewed MAC interface slots can be
/// programmed together. Channel ownership, beacon scheduling, shared TX/RX
/// service and lifecycle arbitration remain outside this module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacStaApReceivePlan {
    station_address: [u8; 6],
    station_bssid: [u8; 6],
    access_point_address: [u8; 6],
}

impl MacStaApReceivePlan {
    /// Build the combined plan from the only station submode observed in a
    /// vendor lifecycle image.
    ///
    /// Vendor-library analysis finds two direct writers of the selector at `g_ic + 0x74` and
    /// both publish zero. The distinct Mode2 register transaction remains
    /// available through [`MacRoleReceivePolicy::Station`] for comparison,
    /// but it is not accepted here as an AP+STA lifecycle meaning.
    pub const fn observed_mode_one(
        station_address: [u8; 6],
        station_bssid: [u8; 6],
        access_point_address: [u8; 6],
    ) -> Self {
        Self {
            station_address,
            station_bssid,
            access_point_address,
        }
    }

    pub const fn station_address(self) -> [u8; 6] {
        self.station_address
    }

    pub const fn station_bssid(self) -> [u8; 6] {
        self.station_bssid
    }

    pub const fn access_point_address(self) -> [u8; 6] {
        self.access_point_address
    }
}

/// The two complete queue-zero submodes selected inside
/// `wifi_set_rx_policy(6)` by the still-untyped `g_ic + 0x74` flag.
///
/// The names intentionally retain the vendor numeric identity. Evidence does
/// not yet prove that mode two means simultaneous AP+STA operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacStaPolicyMode {
    Mode1,
    Mode2,
}

/// One complete, reviewed role-specific receive-policy transaction.
///
/// This finite enum is the shared join key between production HAL calls and
/// bounded vendor verification. It exposes neither vendor jump-table numbers
/// nor arbitrary hardware interface indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacRoleReceivePolicy {
    Station {
        bssid: [u8; 6],
        mode: MacStaPolicyMode,
    },
    AccessPoint {
        address: [u8; 6],
    },
    /// Disable interface-zero receive admission without modifying the access
    /// point context or either interface address.
    StationDisabled,
    /// Disable interface-one receive admission without modifying the station
    /// context or either interface address.
    AccessPointDisabled,
}

/// The receive queue and BSSID bank of interface zero, the station.
const STATION: Index<4> = Index::constant::<0>();

/// The receive queue and BSSID bank of interface one, the SoftAP.
const ACCESS_POINT: Index<4> = Index::constant::<1>();

/// The promiscuous receive queue three.
const SNIFFER: Index<4> = Index::constant::<3>();

/// Publish one interface BSSID through the complete four-edge vendor leaf:
/// disable matching, publish the low word, replace the two high bytes, then
/// make the new address valid.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_mac_set_bssid`,
/// size 0x5a.
fn set_bssid<P: Placement>(bssids: &mut WifiMacBssidPolicy<P>, bank: Index<4>, bssid: [u8; 6]) {
    let [b0, b1, b2, b3, b4, b5] = bssid;
    bssids.modify_bssid_high(bank, |w| {
        w.set_address_check_enable(false);
    });
    // The vendor's whole-word store of the low word: its one field covers
    // every bit.
    bssids.modify_bssid_low(bank, |w| {
        w.set_bytes_0_3(u32::from_le_bytes([b0, b1, b2, b3]));
    });
    bssids.modify_bssid_high(bank, |w| {
        w.set_bssid_high(u16::from_le_bytes([b4, b5]));
    });
    bssids.modify_bssid_high(bank, |w| {
        w.set_address_check_enable(true);
    });
}

/// The six BSSID bytes of one bank.
fn bssid<P: Placement>(bssids: &WifiMacBssidPolicy<P>, bank: Index<4>) -> [u8; 6] {
    let [b0, b1, b2, b3] = bssids.read_bssid_low(bank).bytes_0_3().to_le_bytes();
    let [b4, b5] = bssids.read_bssid_high(bank).bssid_high().to_le_bytes();
    [b0, b1, b2, b3, b4, b5]
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Publish one interface BSSID through the complete four-edge vendor leaf.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_mac_set_bssid`.
    pub fn program_interface_bssid(&mut self, interface: MacInterface, bssid_address: [u8; 6]) {
        set_bssid(
            &mut self.mac.wifi_mac_bssid_policy,
            interface.index(),
            bssid_address,
        );
    }

    pub fn ap_receive_policy_snapshot(&self) -> MacApReceivePolicySnapshot {
        let bssids = &self.mac.wifi_mac_bssid_policy;
        let bssid_high = bssids.read_bssid_high(ACCESS_POINT);
        MacApReceivePolicySnapshot {
            bssid: bssid(bssids, ACCESS_POINT),
            bssid_address_check_enabled: bssid_high.address_check_enable(),
            interface_is_soft_ap: bssid_high.interface_is_soft_ap(),
            interface_rx_policy_enabled: self
                .interface
                .read_address_high(MacInterface::AccessPoint.index())
                .rx_policy_enable(),
        }
    }

    /// Snapshot every register frontier that can suppress associated-STA
    /// beacons while still admitting directed management traffic.
    pub fn sta_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        let bssids = &self.mac.wifi_mac_bssid_policy;
        let bssid_high = bssids.read_bssid_high(STATION);
        let interface = self
            .interface
            .read_address_high(MacInterface::Station.index());

        let bssid = bssid(bssids, STATION);
        let beacon_filter = self
            .mac
            .wifi_mac_sta_beacon_filter
            .read_sta_beacon_filter_control();
        MacStaReceivePolicySnapshot {
            bssid,
            association_id: crate::ieee80211::registers::halfword(bssid_high.association_id()),
            minimum_mpdu_start_spacing: byte(bssid_high.minimum_mpdu_start_spacing()),
            bssid_address_check_enabled: bssid_high.address_check_enable(),
            interface_is_soft_ap: bssid_high.interface_is_soft_ap(),
            interface_rx_policy_enabled: interface.rx_policy_enable(),
            beacon_filter_enabled: beacon_filter.beacon_filter_enable(),
            broadcast_wakeup_enabled: beacon_filter.broadcast_wakeup_enable(),
            unicast_wakeup_enabled: beacon_filter.unicast_wakeup_enable(),
        }
    }

    /// Apply the complete four-queue cold policy transaction from `hal_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0x56..0x90`, and complete `hal_mac_rx_set_policy`.
    /// All 31 fresh-read RMW edges remain separate and in blob order.
    pub fn initialize_cold_receive_policy(&mut self) {
        let filters = &mut self.mac.wifi_mac_rx_filter;
        let bssids = &mut self.mac.wifi_mac_bssid_policy;
        let interfaces = &mut self.interface;

        for (position, context) in COLD_RECEIVE_QUEUE_INTERFACES.into_iter().enumerate() {
            let queue = crate::ieee80211::registers::element::<4>(position);

            filters.modify_policy(queue, |w| {
                w.set_receive_broadcast_check_bssid(true)
                    .set_receive_unicast_check_da(true);
            });
            filters.modify_policy(queue, |w| {
                w.set_receive_management_not_check_bssid(false);
            });
            filters.modify_policy(queue, |w| {
                w.set_dump_unicast_check_da(true)
                    .set_dump_broadcast_check_bssid(true);
            });
            filters.modify_policy(queue, |w| {
                w.set_check_bssid(false)
                    .set_dump_management_not_check_bssid(false);
            });

            if let Some(interface) = context {
                filters.modify_policy(queue, |w| {
                    w.set_receive_management_not_check_bssid(false)
                        .set_pass_beacon(false);
                });
                if interface == MacInterface::AccessPoint {
                    bssids.modify_bssid_high(queue, |w| {
                        w.set_interface_is_soft_ap(true);
                    });
                } else {
                    bssids.modify_bssid_high(queue, |w| {
                        w.set_interface_is_soft_ap(false)
                            .set_address_check_enable(false);
                    });
                }
                filters.modify_policy(queue, |w| {
                    w.set_dump_management_not_check_bssid(false);
                });
                bssids.modify_bssid_high(queue, |w| {
                    w.set_address_check_enable(false);
                });
                clear_cold_interface_address(interfaces, interface);
            }
        }
    }

    /// Switch receive queue zero from scan policy to associated-STA policy.
    ///
    /// SOURCE: complete pinned `libpp.a::hal_mac_rx_set_policy` and
    /// `hal_mac_set_rxq_policy`, reached from the complete
    /// `libnet80211.a::wifi_set_rx_policy` policy-six branch used immediately
    /// before the first live vendor Authentication TX. The prefix is the
    /// complete pinned `libpp.a[hal_sniffer.o]::hal_sniffer_disable` leaf:
    /// unlike the vendor scan lifecycle, the open bootstrap deliberately uses
    /// queue three's promiscuous path, so the associated transition must
    /// restore its normal filtering and hardware auto-ACK policy.
    pub fn apply_sta_link_receive_policy(&mut self, bssid_address: [u8; 6]) {
        self.disable_open_mac_promiscuous_receive();

        self.apply_sta_policy_six(bssid_address, MacStaPolicyMode::Mode1);
    }

    /// Close queue three's promiscuous role admission.
    ///
    /// This is the complete pinned `hal_sniffer_disable` leaf and the explicit
    /// inverse of monitor/scan admission. Role-neutral parking also disables
    /// both ordinary interface policies before the RX ring is stopped.
    pub fn disable_open_mac_promiscuous_receive(&mut self) {
        let filters = &mut self.mac.wifi_mac_rx_filter;

        // SOURCE: `libpp.a[hal_sniffer.o]::hal_sniffer_disable`, size 0x44.
        // Preserve all seven fresh-read RMW edges in blob order.
        filters.modify_policy(SNIFFER, |w| {
            w.set_auto_ack_disable(false);
        });
        filters.modify_policy(SNIFFER, |w| {
            w.set_dump_unicast_check_da(true);
        });
        filters.modify_policy(SNIFFER, |w| {
            w.set_dump_unicast_check_bssid(true);
        });
        filters.modify_policy(SNIFFER, |w| {
            w.set_dump_broadcast_check_bssid(true);
        });
        filters.modify_policy(SNIFFER, |w| {
            w.set_abort_group(true);
        });
        filters.modify_policy(SNIFFER, |w| {
            w.set_receive_unicast_check_bssid(true);
        });
        filters.modify_policy(SNIFFER, |w| {
            w.set_receive_broadcast_check_bssid(true)
                .set_receive_unicast_check_da(true);
        });
    }

    /// Apply exactly `wifi_set_rx_policy(6)` after scan ownership has already
    /// been normalized.
    ///
    /// This finite leaf is separate from
    /// [`Self::apply_sta_link_receive_policy`] so verification does not
    /// incorrectly attribute the open driver's sniffer teardown to the vendor
    /// parent function.
    pub fn apply_sta_policy_six(&mut self, bssid_address: [u8; 6], mode: MacStaPolicyMode) {
        // `ic_set_bssid(0, bssid)` delegates to the complete
        // `hal_mac_set_bssid` leaf.
        set_bssid(&mut self.mac.wifi_mac_bssid_policy, STATION, bssid_address);

        let filters = &mut self.mac.wifi_mac_rx_filter;
        let bssids = &mut self.mac.wifi_mac_bssid_policy;

        // `ic_set_rx_policy(0, mode=0, control=1, management=1)`.
        // Keep every complete-leaf fresh-read RMW edge separate.
        let mode_two = matches!(mode, MacStaPolicyMode::Mode2);
        filters.modify_policy(STATION, |w| {
            w.set_receive_management_not_check_bssid(mode_two)
                .set_pass_beacon(mode_two);
        });
        bssids.modify_bssid_high(STATION, |w| {
            w.set_interface_is_soft_ap(false);
        });
        filters.modify_policy(STATION, |w| {
            w.set_dump_management_not_check_bssid(false);
        });
        bssids.modify_bssid_high(STATION, |w| {
            w.set_address_check_enable(true);
        });
        self.interface
            .modify_address_high(MacInterface::Station.index(), |w| {
                w.set_rx_policy_enable(true);
            });

        // Station policy six finishes with receive-unicast BSSID checking,
        // then dump-unicast BSSID checking, as two ordered RMWs. The complete
        // `ic_set_rx_policy_ubssid_check(0, true)` leaf defines that order;
        // the ESP32-S31 rev0 authentication capture has filter image 0x0001_c387.
        //
        // SOURCE: `libnet80211.a[ieee80211_supplicant.o]`
        // `wifi_set_rx_policy`, jump-table case six at `.L272` through
        // `.L279`; `libpp.a[if_hwctrl.o]`
        // `ic_set_rx_policy_ubssid_check`; a station authentication capture on
        // hardware.
        let filters = &mut self.mac.wifi_mac_rx_filter;
        filters.modify_policy(STATION, |w| {
            w.set_receive_unicast_check_bssid(true);
        });
        filters.modify_policy(STATION, |w| {
            w.set_dump_unicast_check_bssid(true);
        });
    }

    /// Activate the exact first-interface SoftAP receive policy.
    ///
    /// SOURCE: complete `libnet80211.a::wifi_set_rx_policy`, case eight,
    /// executed with `a0=8`. It calls
    /// `ic_set_mac(1, ap)`, `ic_set_bssid(1, ap)` and
    /// `ic_set_rx_policy(1, 0, 1, 1)`. Unlike station policy six it does not
    /// call `ic_set_rx_policy_ubssid_check`.
    pub fn apply_ap_receive_policy(&mut self, access_point: [u8; 6]) {
        self.program_receive_interface_address(MacInterface::AccessPoint, access_point);
        // `ic_set_bssid(1, ap)` / complete `hal_mac_set_bssid`.
        self.program_interface_bssid(MacInterface::AccessPoint, access_point);

        // `ic_set_rx_policy(interface=1, mode=0, control=1, management=1)`.
        self.set_interface_receive_context(ACCESS_POINT, MacInterface::AccessPoint, true);
    }

    /// Disable only the first SoftAP receive context.
    ///
    /// SOURCE: complete `libnet80211.a::wifi_set_rx_policy`, case nine. It
    /// calls only `ic_set_rx_policy(interface=1, mode=0, control=0,
    /// management=0)`. The address and BSSID banks are deliberately retained
    /// so a concurrent station context is not reconstructed or disturbed.
    pub fn disable_ap_receive_policy(&mut self) {
        self.set_interface_receive_context(ACCESS_POINT, MacInterface::AccessPoint, false);
    }

    /// Disable only the infrastructure-station receive context.
    ///
    /// SOURCE: complete `libnet80211.a::wifi_set_rx_policy`, case two. It
    /// calls `ic_set_rx_policy(interface=0, mode=0, control=0,
    /// management=0)` followed by `ic_set_rx_policy_ubssid_check(0, false)`
    /// and therefore preserves interface-one state.
    pub fn disable_sta_receive_policy(&mut self) {
        self.clear_sta_receive_context();
        self.clear_sta_unicast_bssid_check();
    }

    /// Disable both receive contexts: the role-neutral suffix of policy zero.
    ///
    /// SOURCE: complete `libnet80211.a::wifi_set_rx_policy`, case zero, after
    /// its two `ic_set_mac` address republications. It calls
    /// `ic_set_rx_policy(0, 0, 0, 0)`, `ic_set_rx_policy(1, 0, 0, 0)` and
    /// only then `ic_set_rx_policy_ubssid_check(0, false)`, so the station
    /// BSSID-check edges follow the access-point edges.
    pub fn disable_all_role_receive_policies(&mut self) {
        self.clear_sta_receive_context();
        self.disable_ap_receive_policy();
        self.clear_sta_unicast_bssid_check();
    }

    /// `ic_set_rx_policy(interface=0, mode=0, control=0, management=0)`.
    fn clear_sta_receive_context(&mut self) {
        self.set_interface_receive_context(STATION, MacInterface::Station, false);
    }

    /// `ic_set_rx_policy(interface, mode=0, control, management)` of the
    /// station (bank zero, never SoftAP) or the SoftAP (bank one, always
    /// SoftAP): five fresh-read RMW edges in blob order. `enabled` is the
    /// control and management admission, which sets the BSSID check and the
    /// interface's receive-policy enable.
    fn set_interface_receive_context(
        &mut self,
        bank: Index<4>,
        interface: MacInterface,
        enabled: bool,
    ) {
        let soft_ap = interface == MacInterface::AccessPoint;
        let filters = &mut self.mac.wifi_mac_rx_filter;
        let bssids = &mut self.mac.wifi_mac_bssid_policy;
        filters.modify_policy(bank, |w| {
            w.set_receive_management_not_check_bssid(false)
                .set_pass_beacon(false);
        });
        bssids.modify_bssid_high(bank, |w| {
            w.set_interface_is_soft_ap(soft_ap);
        });
        filters.modify_policy(bank, |w| {
            w.set_dump_management_not_check_bssid(false);
        });
        bssids.modify_bssid_high(bank, |w| {
            w.set_address_check_enable(enabled);
        });
        self.interface.modify_address_high(interface.index(), |w| {
            w.set_rx_policy_enable(enabled);
        });
    }

    /// `ic_set_rx_policy_ubssid_check(0, false)`.
    fn clear_sta_unicast_bssid_check(&mut self) {
        let filters = &mut self.mac.wifi_mac_rx_filter;
        filters.modify_policy(STATION, |w| {
            w.set_receive_unicast_check_bssid(false);
        });
        filters.modify_policy(STATION, |w| {
            w.set_dump_unicast_check_bssid(false);
        });
    }

    /// Apply one finite role policy through the production HAL boundary.
    pub fn apply_role_receive_policy(&mut self, policy: MacRoleReceivePolicy) {
        match policy {
            MacRoleReceivePolicy::Station { bssid, mode } => self.apply_sta_policy_six(bssid, mode),
            MacRoleReceivePolicy::AccessPoint { address } => self.apply_ap_receive_policy(address),
            MacRoleReceivePolicy::StationDisabled => self.disable_sta_receive_policy(),
            MacRoleReceivePolicy::AccessPointDisabled => self.disable_ap_receive_policy(),
        }
    }

    /// Program the disjoint STA=0 and SoftAP=1 receive contexts in a fixed
    /// reviewed order.
    ///
    /// The station address is published first, followed by the selected
    /// complete STA policy-six transaction and the complete SoftAP policy-eight
    /// transaction. The component leaves retain their original fresh-read RMW
    /// ordering. This composition intentionally does not touch PHY channel or
    /// scheduler state.
    pub fn apply_sta_ap_receive_plan(&mut self, plan: MacStaApReceivePlan) {
        self.program_receive_interface_address(MacInterface::Station, plan.station_address);
        self.apply_role_receive_policy(MacRoleReceivePolicy::Station {
            bssid: plan.station_bssid,
            mode: MacStaPolicyMode::Mode1,
        });
        self.apply_role_receive_policy(MacRoleReceivePolicy::AccessPoint {
            address: plan.access_point_address,
        });
    }
}

#[cfg(test)]
mod tests;
