//! Generated-PAC ownership for finite receive-policy transitions.

#![forbid(unsafe_code)]

use crate::{MacInterface, WifiRadioRegisters};

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
fn clear_cold_interface_address<P: oer_mmio::Placement>(
    interfaces: &mut crate::generated::wifi_mac_interface_address::WifiMacInterfaceAddress<P>,
    interface: MacInterface,
) {
    interfaces.modify_address_high(interface.index(), |w| {
        w.set_bytes_4_5(0);
    });
}

/// Read-only associated-STA receive-policy evidence.
///
/// Only reviewed semantic fields cross the PAC boundary. Complete policy
/// register images remain private to the generated PAC because several bits
/// are still undocumented and no production decision consumes them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacStaReceivePolicySnapshot {
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub minimum_mpdu_start_spacing: u8,
    pub bssid_address_check_enabled: bool,
    pub interface_is_soft_ap: bool,
    pub interface_rx_policy_enabled: bool,
    pub beacon_filter_control: u8,
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
/// service and lifecycle arbitration remain outside the PAC.
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

impl WifiRadioRegisters {
    /// Publish one interface BSSID through the complete four-edge vendor leaf.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_mac_set_bssid`.
    pub fn program_interface_bssid(&mut self, interface: MacInterface, bssid_address: [u8; 6]) {
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let index = interface.bits() as usize;
        let bssid = bssids.bssid_high(index);
        bssid.modify(|_, w| w.address_check_enable().clear_bit());
        crate::svd::zero_based_field_write::mac_bssid_address_low(
            bssids,
            index,
            u32::from_le_bytes([
                bssid_address[0],
                bssid_address[1],
                bssid_address[2],
                bssid_address[3],
            ]),
        );
        bssid.modify(|_, w| {
            w.bssid_high()
                .set(u16::from_le_bytes([bssid_address[4], bssid_address[5]]))
        });
        bssid.modify(|_, w| w.address_check_enable().set_bit());
    }

    pub fn ap_receive_policy_snapshot(&self) -> MacApReceivePolicySnapshot {
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let bssid_low = bssids.bssid_low(1).read().bytes_0_3().bits().to_le_bytes();
        let bssid_high = bssids.bssid_high(1).read();
        let high = bssid_high.bssid_high().bits().to_le_bytes();
        MacApReceivePolicySnapshot {
            bssid: [
                bssid_low[0],
                bssid_low[1],
                bssid_low[2],
                bssid_low[3],
                high[0],
                high[1],
            ],
            bssid_address_check_enabled: bssid_high.address_check_enable().bit_is_set(),
            interface_is_soft_ap: bssid_high.interface_is_soft_ap().bit_is_set(),
            interface_rx_policy_enabled: self
                .peripherals
                .wifi_mac_interface
                .wifi_mac_interface_address
                .read_address_high(MacInterface::AccessPoint.index())
                .rx_policy_enable(),
        }
    }

    /// Snapshot every register frontier that can suppress associated-STA
    /// beacons while still admitting directed management traffic.
    pub fn sta_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let bssid_low = bssids.bssid_low(0).read().bytes_0_3().bits();
        let bssid_high = bssids.bssid_high(0).read();
        let interface = self
            .peripherals
            .wifi_mac_interface
            .wifi_mac_interface_address
            .read_address_high(MacInterface::Station.index());
        let low = bssid_low.to_le_bytes();
        let high = bssid_high.bssid_high().bits().to_le_bytes();

        MacStaReceivePolicySnapshot {
            bssid: [low[0], low[1], low[2], low[3], high[0], high[1]],
            association_id: bssid_high.association_id().bits(),
            minimum_mpdu_start_spacing: bssid_high.minimum_mpdu_start_spacing().bits(),
            bssid_address_check_enabled: bssid_high.address_check_enable().bit_is_set(),
            interface_is_soft_ap: bssid_high.interface_is_soft_ap().bit_is_set(),
            interface_rx_policy_enabled: interface.rx_policy_enable(),
            beacon_filter_control: self
                .peripherals
                .wifi_mac
                .wifi_mac_sta_beacon_filter
                .control()
                .read()
                .bits_0_2_opaque()
                .bits(),
        }
    }

    /// Apply the complete four-queue cold policy transaction from `hal_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0x56..0x90`, and complete `hal_mac_rx_set_policy`.
    /// All 31 fresh-read RMW edges remain separate and in blob order.
    pub fn initialize_cold_receive_policy(&mut self) {
        let filters = &self.peripherals.wifi_mac.wifi_mac_rx_filter;
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let interfaces = &mut self
            .peripherals
            .wifi_mac_interface
            .wifi_mac_interface_address;

        for (queue, context) in COLD_RECEIVE_QUEUE_INTERFACES.into_iter().enumerate() {
            let filter = filters.policy(queue);

            filter.modify(|_, w| {
                w.receive_broadcast_check_bssid()
                    .set_bit()
                    .receive_unicast_check_da()
                    .set_bit()
            });
            filter.modify(|_, w| w.receive_management_not_check_bssid().clear_bit());
            filter.modify(|_, w| {
                w.dump_unicast_check_da()
                    .set_bit()
                    .dump_broadcast_check_bssid()
                    .set_bit()
            });
            filter.modify(|_, w| {
                w.check_bssid()
                    .clear_bit()
                    .dump_management_not_check_bssid()
                    .clear_bit()
            });

            if let Some(interface) = context {
                let bssid = bssids.bssid_high(queue);
                filter.modify(|_, w| {
                    w.receive_management_not_check_bssid()
                        .clear_bit()
                        .pass_beacon()
                        .clear_bit()
                });
                if queue == 1 {
                    bssid.modify(|_, w| w.interface_is_soft_ap().set_bit());
                } else {
                    bssid.modify(|_, w| {
                        w.interface_is_soft_ap()
                            .clear_bit()
                            .address_check_enable()
                            .clear_bit()
                    });
                }
                filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
                bssid.modify(|_, w| w.address_check_enable().clear_bit());
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
        let sniffer = self.peripherals.wifi_mac.wifi_mac_rx_filter.policy(3);

        // SOURCE: `libpp.a[hal_sniffer.o]::hal_sniffer_disable`, size 0x44.
        // Preserve all seven fresh-read RMW edges in blob order.
        sniffer.modify(|_, w| w.auto_ack_disable().clear_bit());
        sniffer.modify(|_, w| w.dump_unicast_check_da().set_bit());
        sniffer.modify(|_, w| w.dump_unicast_check_bssid().set_bit());
        sniffer.modify(|_, w| w.dump_broadcast_check_bssid().set_bit());
        sniffer.modify(|_, w| w.abort_group().set_bit());
        sniffer.modify(|_, w| w.receive_unicast_check_bssid().set_bit());
        sniffer.modify(|_, w| {
            w.receive_broadcast_check_bssid()
                .set_bit()
                .receive_unicast_check_da()
                .set_bit()
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
        let filter = self.peripherals.wifi_mac.wifi_mac_rx_filter.policy(0);
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let bssid = bssids.bssid_high(0);
        let interface = &mut self
            .peripherals
            .wifi_mac_interface
            .wifi_mac_interface_address;

        // `ic_set_bssid(0, bssid)` delegates to the complete
        // `hal_mac_set_bssid` leaf. Keep its four hardware edges ordered:
        // disable matching, publish the low word, replace the two high bytes,
        // then make the new address valid.
        //
        // SOURCE: complete pinned `libpp.a[hal_mac.o]`
        // `hal_mac_set_bssid`, size 0x5a.
        bssid.modify(|_, w| w.address_check_enable().clear_bit());
        crate::svd::zero_based_field_write::mac_bssid_address_low(
            bssids,
            0,
            u32::from_le_bytes([
                bssid_address[0],
                bssid_address[1],
                bssid_address[2],
                bssid_address[3],
            ]),
        );
        bssid.modify(|_, w| {
            w.bssid_high()
                .set(u16::from_le_bytes([bssid_address[4], bssid_address[5]]))
        });
        bssid.modify(|_, w| w.address_check_enable().set_bit());

        // `ic_set_rx_policy(0, mode=0, control=1, management=1)`.
        // Keep every complete-leaf fresh-read RMW edge separate.
        filter.modify(|_, w| match mode {
            MacStaPolicyMode::Mode1 => w
                .receive_management_not_check_bssid()
                .clear_bit()
                .pass_beacon()
                .clear_bit(),
            MacStaPolicyMode::Mode2 => w
                .receive_management_not_check_bssid()
                .set_bit()
                .pass_beacon()
                .set_bit(),
        });
        bssid.modify(|_, w| w.interface_is_soft_ap().clear_bit());
        filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
        bssid.modify(|_, w| w.address_check_enable().set_bit());
        interface.modify_address_high(MacInterface::Station.index(), |w| {
            w.set_rx_policy_enable(true);
        });

        // Station policy six finishes with receive-unicast BSSID checking,
        // then dump-unicast BSSID checking, as two ordered RMWs. The complete
        // `ic_set_rx_policy_ubssid_check(0, true)` leaf defines that order;
        // the ESP32-S31 rev0 authentication capture has filter image 0x0001_c387.
        //
        // SOURCE: `libnet80211.a[ieee80211_supplicant.o]`
        // `wifi_set_rx_policy`, jump-table case six at `.L255` through
        // `.L262`; `libpp.a[if_hwctrl.o]`
        // `ic_set_rx_policy_ubssid_check`; a station authentication capture on
        // hardware.
        filter.modify(|_, w| w.receive_unicast_check_bssid().set_bit());
        filter.modify(|_, w| w.dump_unicast_check_bssid().set_bit());
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

        let filter = self.peripherals.wifi_mac.wifi_mac_rx_filter.policy(1);
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let bssid = bssids.bssid_high(1);
        let interface = &mut self
            .peripherals
            .wifi_mac_interface
            .wifi_mac_interface_address;

        // `ic_set_rx_policy(interface=1, mode=0, control=1, management=1)`.
        filter.modify(|_, w| {
            w.receive_management_not_check_bssid()
                .clear_bit()
                .pass_beacon()
                .clear_bit()
        });
        bssid.modify(|_, w| w.interface_is_soft_ap().set_bit());
        filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
        bssid.modify(|_, w| w.address_check_enable().set_bit());
        interface.modify_address_high(MacInterface::AccessPoint.index(), |w| {
            w.set_rx_policy_enable(true);
        });
    }

    /// Disable only the first SoftAP receive context.
    ///
    /// SOURCE: complete `libnet80211.a::wifi_set_rx_policy`, case nine. It
    /// calls only `ic_set_rx_policy(interface=1, mode=0, control=0,
    /// management=0)`. The address and BSSID banks are deliberately retained
    /// so a concurrent station context is not reconstructed or disturbed.
    pub fn disable_ap_receive_policy(&mut self) {
        let filter = self.peripherals.wifi_mac.wifi_mac_rx_filter.policy(1);
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let bssid = bssids.bssid_high(1);
        let interface = &mut self
            .peripherals
            .wifi_mac_interface
            .wifi_mac_interface_address;

        filter.modify(|_, w| {
            w.receive_management_not_check_bssid()
                .clear_bit()
                .pass_beacon()
                .clear_bit()
        });
        bssid.modify(|_, w| w.interface_is_soft_ap().set_bit());
        filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
        bssid.modify(|_, w| w.address_check_enable().clear_bit());
        interface.modify_address_high(MacInterface::AccessPoint.index(), |w| {
            w.set_rx_policy_enable(false);
        });
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
        let filter = self.peripherals.wifi_mac.wifi_mac_rx_filter.policy(0);
        let bssids = &self.peripherals.wifi_mac.wifi_mac_bssid_policy;
        let bssid = bssids.bssid_high(0);
        let interface = &mut self
            .peripherals
            .wifi_mac_interface
            .wifi_mac_interface_address;

        filter.modify(|_, w| {
            w.receive_management_not_check_bssid()
                .clear_bit()
                .pass_beacon()
                .clear_bit()
        });
        bssid.modify(|_, w| w.interface_is_soft_ap().clear_bit());
        filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
        bssid.modify(|_, w| w.address_check_enable().clear_bit());
        interface.modify_address_high(MacInterface::Station.index(), |w| {
            w.set_rx_policy_enable(false);
        });
    }

    /// `ic_set_rx_policy_ubssid_check(0, false)`.
    fn clear_sta_unicast_bssid_check(&mut self) {
        let filter = self.peripherals.wifi_mac.wifi_mac_rx_filter.policy(0);
        filter.modify(|_, w| w.receive_unicast_check_bssid().clear_bit());
        filter.modify(|_, w| w.dump_unicast_check_bssid().clear_bit());
    }

    /// Apply one finite role policy through the production PAC boundary.
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
