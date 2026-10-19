//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use crate::startup::WifiStartReport;

use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiClocksOn},
    owner::{MacInterruptSetup, RadioAccess},
    types::MacInterruptEnableState,
};

use oer_esp32s31_ieee80211_mac::sta_ap_registers::disable_all_role_receive_registers;
use oer_esp32s31_radio_client::{
    DomainHealth, Leaving, Left, Member, MembershipCell, Tracked,
    wifi::{Wifi, WifiLeave, WifiLeft, WifiReturned},
};

use oer_ieee80211_mac::channel::WifiChannel;

/// Evidence captured while closing the cold polling interrupt phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiRuntimeTransitionReport {
    /// Mask published by the common MAC initializer before task-side routing.
    pub cold_interrupt_mask: MacInterruptEnableState,
}

/// Common stopped Wi-Fi owner after cold MAC initialization.
///
/// No Wi-Fi role owns DMA or an installed CPU interrupt route in this state.
/// A station, AP or standalone monitor must consume this value to begin its
/// own finite runtime epoch and return the same ownership frontier only after
/// both DMA and interrupt routing have acknowledged their stopped edges.
///
/// `W` is Wi-Fi's own MAC platform. The shared PHY, common power, the clock
/// sources and coexistence stay with the radio system.
///
/// Its tracked membership of the shared PHY domain, which the bring-up's
/// `track` issued, stays inside it until [`WifiStopped::release`] gives it
/// back through `leave`:
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_driver::runtime::WifiStopped;
/// use oer_ieee80211_mac::channel::WifiChannel;
/// fn channel(stopped: &WifiStopped<()>) -> WifiChannel {
///     stopped.current_channel()
/// }
/// ```
///
/// ```compile_fail,E0616
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_driver::runtime::WifiStopped;
/// use oer_esp32s31_radio_client::{Member, Tracked, wifi::Wifi};
/// fn membership(stopped: &WifiStopped<()>) -> &Member<Wifi, Tracked> {
///     &stopped.member
/// }
/// ```
///
/// It leaves the radio only stopped, through its cell:
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_driver::runtime::WifiStopped;
/// use oer_esp32s31_radio_client::{MembershipCell, wifi::Wifi};
/// async fn leave(stopped: WifiStopped<()>, cell: MembershipCell<Wifi>) {
///     let _ = stopped.release(cell).await;
/// }
/// ```
pub struct WifiStopped<W> {
    platform: W,
    registers: RadioAccess,
    interrupt_setup: MacInterruptSetup,
    clocks: WifiClocksOn,
    member: Member<Wifi, Tracked>,
    start_report: WifiStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

/// A stopped Wi-Fi client that could not leave because its MAC or RX walker
/// is still active; it is returned unchanged with its cell.
#[must_use = "a failed Wi-Fi release still owns the Wi-Fi partition"]
pub struct WifiReleaseFailure<W> {
    pub error: oer_esp32s31_hal::owner::maintenance::Error,
    pub stopped: WifiStopped<W>,
}

/// Wi-Fi left the radio system.
pub struct WifiReleased<W> {
    /// The partition, cold on success; at the stage the exit reached when
    /// the domain is poisoned.
    pub wifi: WifiReturned,
    pub platform: W,
    pub domain: DomainHealth,
}

impl<W> WifiStopped<W> {
    /// Leave the radio system: confirm the MAC stopped, then `leave` through
    /// `cell`, which disables Wi-Fi's receive path, leaves the PHY client
    /// set, releases the module clocks and initialized status, coexistence
    /// and common power. The last member's leave closes RF.
    ///
    /// # Errors
    ///
    /// The MAC or the RX walker is still active; nothing was posted.
    pub async fn release(
        self,
        cell: MembershipCell<Wifi>,
    ) -> (
        MembershipCell<Wifi>,
        Result<WifiReleased<W>, WifiReleaseFailure<W>>,
    ) {
        let Self {
            platform,
            registers,
            interrupt_setup,
            clocks,
            member,
            start_report,
            transition_report,
            current_channel,
        } = self;
        let (registers, interrupt_setup) = match registers.try_confirm_phy_stopped(interrupt_setup)
        {
            Ok(frontier) => frontier,
            Err(failure) => {
                return (
                    cell,
                    Err(WifiReleaseFailure {
                        error: failure.error,
                        stopped: Self {
                            platform,
                            registers: failure.registers,
                            interrupt_setup: failure.interrupts,
                            clocks,
                            member,
                            start_report,
                            transition_report,
                            current_channel,
                        },
                    }),
                );
            }
        };
        let wifi = WifiClocked::from_running(registers, interrupt_setup, clocks);
        let (cell, left) = cell
            .leave(Leaving::Tracked(member), WifiLeave { wifi })
            .await;
        let Left {
            payload: WifiLeft { wifi },
            domain,
        } = left;
        (
            cell,
            Ok(WifiReleased {
                wifi,
                platform,
                domain,
            }),
        )
    }

    /// Reports from the PHY and common MAC transitions.
    pub const fn start_report(&self) -> WifiStartReport {
        self.start_report
    }

    pub const fn transition_report(&self) -> WifiRuntimeTransitionReport {
        self.transition_report
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    /// Borrow the role-neutral radio state for stopped-only operations.
    pub fn radio_mut(&mut self) -> (oer_esp32s31_hal::ieee80211::mac::WifiMacHal<'_>, &mut W) {
        (self.registers.wifi_mac_hal(), &mut self.platform)
    }

    /// Move the exact common ownership frontier into one role runtime.
    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> WifiRuntimeParts<W> {
        WifiRuntimeParts {
            platform: self.platform,
            registers: self.registers,
            interrupt_setup: self.interrupt_setup,
            context: WifiRuntimeContext {
                clocks: self.clocks,
                member: self.member,
                start_report: self.start_report,
                transition_report: self.transition_report,
                current_channel: self.current_channel,
            },
        }
    }
}

/// Atomic transfer object between the common stopped owner and one role.
///
/// Keeping the membership, MMIO, interrupt setup and platform ownership
/// together avoids a public constructor which could combine pieces from
/// unrelated epochs.
#[doc(hidden)]
pub struct WifiRuntimeParts<W> {
    pub platform: W,
    pub registers: RadioAccess,
    pub interrupt_setup: MacInterruptSetup,
    pub context: WifiRuntimeContext,
}

/// Common Wi-Fi state retained beside one materialized role.
///
/// Register ownership and the interrupt setup token deliberately do not live
/// in this value. A role can reconstruct [`WifiStopped`] only after
/// its DMA/task graph returns the exact runtime owner and its interrupt
/// route returns the exact [`MacInterruptSetup`].
#[doc(hidden)]
pub struct WifiRuntimeContext {
    clocks: WifiClocksOn,
    member: Member<Wifi, Tracked>,
    start_report: WifiStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl WifiRuntimeContext {
    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.current_channel = channel;
    }

    /// Reconstruct the role-neutral owner from independently proven DMA/task
    /// and interrupt-route return edges.
    pub fn into_stopped<W>(
        self,
        platform: W,
        registers: RadioAccess,
        interrupt_setup: MacInterruptSetup,
    ) -> WifiStopped<W> {
        WifiStopped {
            platform,
            registers,
            interrupt_setup,
            clocks: self.clocks,
            member: self.member,
            start_report: self.start_report,
            transition_report: self.transition_report,
            current_channel: self.current_channel,
        }
    }
}

/// Unique Wi-Fi platform owner while any finite Wi-Fi role graph is active.
///
/// RX DMA, register and interrupt capabilities move independently into the
/// concrete epoch. The owner is deliberately role-neutral: a same-channel
/// STA+AP graph still has exactly one Wi-Fi client and must not manufacture
/// separate station and access-point owners around it.
pub struct WifiRoleOwner<W> {
    platform: W,
    context: WifiRuntimeContext,
}

impl<W> WifiRoleOwner<W> {
    pub const fn start_report(&self) -> WifiStartReport {
        self.context.start_report
    }

    pub const fn transition_report(&self) -> WifiRuntimeTransitionReport {
        self.context.transition_report
    }

    pub fn platform_mut(&mut self) -> &mut W {
        &mut self.platform
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.context.set_current_channel(channel);
    }

    pub fn context_mut(&mut self) -> &mut WifiRuntimeContext {
        &mut self.context
    }

    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> (W, WifiRuntimeContext) {
        (self.platform, self.context)
    }

    #[doc(hidden)]
    pub fn from_runtime_parts(platform: W, context: WifiRuntimeContext) -> Self {
        Self { platform, context }
    }

    /// Return the role-neutral owner after the role's DMA/task graph and
    /// interrupt route returned their exact owners.
    pub fn into_stopped<L>(
        self,
        registers: RadioAccess,
        interrupt_setup: MacInterruptSetup,
        resources: L,
    ) -> WifiRoleStopped<W, L> {
        WifiRoleStopped {
            wifi: self
                .context
                .into_stopped(self.platform, registers, interrupt_setup),
            resources,
        }
    }
}

/// One materialized role graph's owners.
pub struct WifiRoleMaterialized<W, L> {
    pub owner: WifiRoleOwner<W>,
    pub registers: RadioAccess,
    pub interrupt_setup: MacInterruptSetup,
    pub resources: L,
}

/// Cleanly dematerialized finite role graph.
pub struct WifiRoleStopped<W, L> {
    pub wifi: WifiStopped<W>,
    pub resources: L,
}

/// Consume the common stopped owner before starting any finite role graph.
pub fn materialize_esp32s31_wifi_role<W, L>(
    wifi: WifiStopped<W>,
    resources: L,
) -> WifiRoleMaterialized<W, L> {
    let WifiRuntimeParts {
        platform,
        registers,
        interrupt_setup,
        context,
    } = wifi.into_runtime_parts();
    WifiRoleMaterialized {
        owner: WifiRoleOwner { platform, context },
        registers,
        interrupt_setup,
        resources,
    }
}

/// Close the cold polling phase and enter the reusable stopped-runtime state.
///
/// This is the only normal conversion from the tracked client with its MAC
/// up. It masks and acknowledges cold interrupt state before exposing the
/// setup token used by a finite task-owned interrupt epoch. It touches Wi-Fi
/// MAC registers only.
pub(crate) fn enter_esp32s31_wifi_runtime<W>(
    mut clocked: WifiClocked,
    member: Member<Wifi, Tracked>,
    platform: W,
    start_report: WifiStartReport,
) -> WifiStopped<W> {
    let cold_interrupt_mask = clocked.close_cold_interrupt_phase();
    let (mut registers, interrupt_setup, clocks) = clocked.into_running();
    // Cold `wifi_set_rx_policy(0)` first publishes both interface addresses,
    // then disables their receive policies. Our cold address transaction is
    // already complete; finish that exact role-neutral suffix before exposing
    // a stopped runtime. Scan/monitor subsequently own only queue three's
    // explicit promiscuous admission, while STA/AP reopen their own context.
    {
        let mut mac = registers.wifi_mac_hal();
        disable_all_role_receive_registers(&mut mac);
        // Complete the role-neutral half of the vendor no-power-save lifecycle.
        // The first role arms its replacement RX ring while this request remains
        // asserted and resumes the frontend only after those credits are live.
        mac.request_channel_stop();
    }
    WifiStopped {
        platform,
        registers,
        interrupt_setup,
        clocks,
        member,
        start_report,
        transition_report: WifiRuntimeTransitionReport {
            cold_interrupt_mask,
        },
        current_channel: start_report.initial_channel,
    }
}
