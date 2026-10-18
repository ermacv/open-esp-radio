#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Linked placement fixture, never a bootable application or HIL evidence.
//! A synthetic separate executor retains the same scope/IRQ monomorphization;
//! all register transactions are the real typed HAL operations.
//!
//! `DrainBound::from_maxima::<100, 20, 30>()` contains fixture placeholders
//! (microseconds), not measured production bounds. The S31 implementation step
//! must supply step/ISR maxima with measurement provenance and a justified
//! margin, together with an independent monotonic clock.

#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(target_arch = "riscv32")]
mod image {
    use core::{
        convert::Infallible,
        future::Future,
        pin::pin,
        sync::atomic::{AtomicU32, Ordering},
        task::{Context, RawWaker, RawWakerVTable, Waker},
    };
    use oer_esp32s31_hal::{
        owner::{RadioRuntimeOwner, validation_mac_interrupt_setup},
        types::MacInterruptMask,
    };
    use oer_esp32s31_ieee80211_form::scope::commands::*;
    use oer_esp32s31_ieee80211_form::scope::*;
    use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, InactiveEpoch, Moderated};
    use oer_ieee80211_lower_mac::{Ieee80211Capabilities, LowerMacCapabilities, StationReceive};
    use oer_time::Instant;

    oer_esp32s31_ieee80211_form::declare_scope_slot!(static SLOT);

    #[allow(
        unsafe_code,
        reason = "the fixture's executor data must survive cache loss"
    )]
    #[unsafe(link_section = ".critical.data.open_radio_irq")]
    static WAKES: AtomicU32 = AtomicU32::new(0);

    #[allow(
        unsafe_code,
        reason = "the fixture's immutable waker vtable belongs in SRAM"
    )]
    #[unsafe(link_section = ".critical.rodata.open_radio_irq")]
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop_waker);

    #[allow(unsafe_code, reason = "the fixture's IRQ callbacks belong in SRAM")]
    #[unsafe(link_section = ".rwtext.open_radio_irq")]
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::from_ref(&WAKES).cast(), &VTABLE)
    }
    #[allow(unsafe_code, reason = "the fixture's IRQ callbacks belong in SRAM")]
    #[unsafe(link_section = ".rwtext.open_radio_irq")]
    fn wake(_: *const ()) {
        WAKES.fetch_add(1, Ordering::Relaxed);
    }
    #[allow(unsafe_code, reason = "the fixture's IRQ callbacks belong in SRAM")]
    #[unsafe(link_section = ".rwtext.open_radio_irq")]
    fn drop_waker(_: *const ()) {}

    // A synthetic clock for this link-only fixture. The production S31
    // composition must bind its independent monotonic clock and measured bound.
    static CLOCK: AtomicU32 = AtomicU32::new(0);
    fn now(_: &()) -> Instant {
        Instant::from_micros(u64::from(CLOCK.fetch_add(1, Ordering::Relaxed)))
    }
    fn mask(_: &()) {}
    fn acknowledge(_: &()) {}
    fn enable(_: &()) -> Result<(), Infallible> {
        Ok(())
    }
    struct Scheduler;
    impl WakeScheduler<()> for Scheduler {
        #[allow(unsafe_code, reason = "the scheduler's IRQ callback belongs in SRAM")]
        #[unsafe(link_section = ".rwtext.open_radio_irq")]
        fn schedule(&self, _: &(), _: WakePlan) {
            WAKES.fetch_add(1, Ordering::Relaxed);
        }
    }
    type Backing =
        ScopeStorage<RadioRuntimeOwner, ActiveEpoch<Moderated>, (), Infallible, Scheduler>;
    #[allow(
        unsafe_code,
        reason = "one boot-owned permanent SRAM allocation, transferred once by _start"
    )]
    #[unsafe(link_section = ".critical.data.open_radio_scope")]
    static mut BACKING: Backing = Backing::new();

    struct Caps;
    impl Ieee80211Capabilities for Caps {
        type SppScope = oer_ieee80211_mac::security::spp::Interface;
        const CAPS: LowerMacCapabilities =
            oer_esp32s31_ieee80211::lower_mac::esp32s31_lower_mac_capabilities(2048);
    }
    // Representative ownership envelope: a context word and three retained
    // SRAM lease words (header and two bodies). No hardware SG claim is made.
    type Commands<'id> = CommandStorage<'id, Caps, u32, u32, usize, [usize; 3], 2, 2, 4, 15>;
    #[allow(
        unsafe_code,
        reason = "export the target ABI layout for the linked cost measurement"
    )]
    #[unsafe(no_mangle)]
    static S31_COMMAND_STORAGE_BYTES: u32 = core::mem::size_of::<Commands<'static>>() as u32;

    type BridgeHeld<'id> =
        bridge::Held<'id, Caps, u32, u32, usize, [usize; 3], 2, 2, 4, 15, RadioRuntimeOwner>;
    #[allow(unsafe_code, reason = "export target ABI sizes for the bridge model")]
    #[unsafe(no_mangle)]
    static S31_BRIDGE_HELD_BYTES: u32 = core::mem::size_of::<BridgeHeld<'static>>() as u32;
    #[allow(unsafe_code, reason = "export target ABI sizes for the bridge model")]
    #[unsafe(no_mangle)]
    static S31_BRIDGE_EXECUTOR_BYTES: u32 =
        core::mem::size_of::<bridge::Executor<RadioRuntimeOwner, 4, 15>>() as u32;
    #[allow(
        unsafe_code,
        reason = "composition-owned command records require permanent SRAM"
    )]
    #[unsafe(link_section = ".critical.data.open_radio_commands")]
    static mut BRIDGE_BACKING: bridge::ExecutionStorage<4, 15> = bridge::ExecutionStorage::new();

    async fn command_workload() {
        oer_radio_port::branded::<CommandPools<2, 2>, _>(async |mint| {
            let r = mint.into_pools();
            let mut storage = Commands::new(r.bridge);
            let (mut setup, mut receiver) = storage.split(r.issuer, r.enable, r.disable);
            setup.configure(Configuration::Station {
                permission: r.station.configure.with_context(core::hint::black_box(7)),
                receive: StationReceive::Off,
            });
            let phase = setup.enable();
            let configure = receiver.next().await;
            core::hint::black_box(configure);
            if let SetupCommand::Enable(enable) = receiver.next().await {
                let mut receiver = receiver.enabled(enable);
                let disabling = phase.disable();
                if let PhaseCommand::Disable(disable) = receiver.next().await {
                    let (_receiver, reply) = receiver.disabled(disable);
                    let _setup = disabling.finished(reply);
                }
            }
            core::future::pending::<()>().await;
            // Retain the actual queue storage in the measured caller future.
            core::hint::black_box(storage);
        })
        .await;
    }
    fn stop(radio: &mut RadioRuntimeOwner, _: &mut IrqControl<'_>) -> StopOutcome {
        core::hint::black_box(radio);
        // The placement audit stops no DMA and claims no quiescence.
        StopOutcome::Unconfirmed
    }
    fn step(
        radio: &mut RadioRuntimeOwner,
        _: &mut IrqControl<'_>,
        events: DriverEvents,
        signals: DriverStatus,
    ) -> DriverStep {
        core::hint::black_box((radio, events.mac.count(), events.power));
        if signals == DriverStatus::RetireRequested {
            DriverStep::Retired
        } else {
            DriverStep::Wake(WakePlan::until(
                Instant::from_micros(0),
                Instant::from_micros(1),
                WakeSources::CommandsAndInterrupts,
            ))
        }
    }

    #[allow(
        unsafe_code,
        reason = "retained hard-IRQ root for the linked graph audit"
    )]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".rwtext.open_radio_irq")]
    pub fn s31_form_mac_irq() {
        core::hint::black_box(SLOT.mac_interrupt());
    }
    #[allow(
        unsafe_code,
        reason = "retained power-IRQ root for the linked graph audit"
    )]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".rwtext.open_radio_irq")]
    pub fn s31_form_power_irq() {
        core::hint::black_box(SLOT.power_interrupt());
    }

    struct CriticalSection;
    critical_section::set_impl!(CriticalSection);
    #[allow(
        unsafe_code,
        reason = "the link-only fixture serializes every call in one root"
    )]
    // SAFETY: this image is analyzed, never executed or flashed. Its explicit
    // root calls each executor entry sequentially; it starts no CPU interrupt.
    unsafe impl critical_section::Impl for CriticalSection {
        unsafe fn acquire() -> critical_section::RawRestoreState {}
        unsafe fn release(_: critical_section::RawRestoreState) {}
    }

    #[allow(
        unsafe_code,
        reason = "retained root of the link-only execution fixture"
    )]
    #[unsafe(no_mangle)]
    pub fn scope_main() -> ! {
        core::hint::black_box(&S31_COMMAND_STORAGE_BYTES);
        core::hint::black_box(&S31_BRIDGE_HELD_BYTES);
        core::hint::black_box(&S31_BRIDGE_EXECUTOR_BYTES);
        core::hint::black_box(&raw mut BRIDGE_BACKING);
        let mut latch = ScopeReturnLatch::new();
        let route = ScopeRoute::new((), &SLOT, mask, acknowledge, enable, Scheduler, now);
        let radio = RadioRuntimeOwner::claim_for_validation();
        let interrupts = InactiveEpoch::new(validation_mac_interrupt_setup())
            .activate_rx_moderated(MacInterruptMask::COLD_RX);
        // SAFETY: this link-only fixture has one slot and serialized explicit
        // dispatch; it is never run as a hardware image. Its bounded step/stop
        // and routing callbacks model that boundary without reentry or wakes.
        // SAFETY: _start calls scope_main once. No other path creates a
        // reference to this static; the affine pinned lease never exposes it.
        let lease = ScopeStorage::pin_static(unsafe { &mut *core::ptr::addr_of_mut!(BACKING) });
        // SAFETY: the exact callback and routing premises are stated above.
        let scope = unsafe {
            DriverScope::new(
                lease,
                radio,
                interrupts,
                DriverCallbacks::new(step, stop),
                route,
                DrainBound::from_maxima::<100, 20, 30>(),
                &mut latch,
            )
        };
        let mut future = pin!(scope.run((), async |(), access| {
            core::hint::black_box(access.irq());
            command_workload().await;
        }));
        // SAFETY: the vtable touches only the independent static atomic; all
        // clone/wake/drop operations remain valid after the scope is forgotten.
        let waker = unsafe { Waker::from_raw(clone(core::ptr::null())) };
        let mut cx = Context::from_waker(&waker);
        loop {
            core::hint::black_box(future.as_mut().poll(&mut cx));
            s31_form_mac_irq();
            s31_form_power_irq();
            // SAFETY: this is the only serialized driver entry of the fixture.
            unsafe {
                SLOT.dispatch();
            }
        }
    }

    // The future owns only the lease. Registration, Driver and IRQ handoffs
    // are in BACKING. The ISR/executor stack still needs a linked SRAM range.
    #[allow(
        unsafe_code,
        reason = "link-only entry establishes the audited SRAM stack"
    )]
    #[unsafe(naked)]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn _start() -> ! {
        core::arch::naked_asm!("la sp, __scope_stack_top", "j scope_main");
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
}
