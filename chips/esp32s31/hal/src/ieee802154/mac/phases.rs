//! Owned startup/teardown phases; operation order is executed by the family driver.
use super::*;
use core::marker::PhantomData;
use oer_esp32s31_pac::ieee802154::command::Opcode;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154InterruptStartup, Ieee802154InterruptTeardown,
};
use oer_mmio::Placement;

/// S31 implementation of the consuming startup register port.
pub struct Ieee802154StartupPort<P = MacPlacement, E = EtmPlacement, B = RxInfoPlacement>(
    PhantomData<(P, E, B)>,
);
/// An unbound startup phase; no register, task or IRQ extraction is public.
#[must_use = "complete the startup phases before binding"]
pub struct Ieee802154Startup<P: Placement, E: Placement, B: Placement, const STEP: u8> {
    task: Ieee802154TaskOwner<P, E, B>,
    setup: Ieee802154InterruptSetupOwner<P>,
}
impl<P: Placement, E: Placement, B: Placement, const STEP: u8> Ieee802154Startup<P, E, B, STEP> {
    fn advance<const NEXT: u8>(self) -> Ieee802154Startup<P, E, B, NEXT> {
        Ieee802154Startup {
            task: self.task,
            setup: self.setup,
        }
    }
}
impl<P: Placement, E: Placement, B: Placement> Ieee802154InterruptStartup
    for Ieee802154StartupPort<P, E, B>
{
    type Task = Ieee802154TaskOwner<P, E, B>;
    type Setup = Ieee802154InterruptSetupOwner<P>;
    type Masked = Ieee802154Startup<P, E, B, 1>;
    type TxAborts = Ieee802154Startup<P, E, B, 2>;
    type RxAborts = Ieee802154Startup<P, E, B, 3>;
    type Ordered = Ieee802154Startup<P, E, B, 4>;
    type Stopped = Ieee802154Startup<P, E, B, 5>;
    type Timer0Stopped = Ieee802154Startup<P, E, B, 6>;
    type TimersStopped = Ieee802154Startup<P, E, B, 7>;
    type DiscardReady = Ieee802154Startup<P, E, B, 8>;
    type Discarded = Ieee802154Startup<P, E, B, 9>;
    type Output = (
        Ieee802154ArmedTaskOwner<P, E, B>,
        Ieee802154InterruptOwner<P>,
    );
    fn mask_events(setup: Self::Setup, mut task: Self::Task) -> Self::Masked {
        task.mask_all_events();
        Ieee802154Startup { task, setup }
    }
    fn runtime_tx_aborts(mut phase: Self::Masked) -> Self::TxAborts {
        phase.task.enable_only_runtime_tx_aborts();
        phase.advance()
    }
    fn runtime_rx_aborts(mut phase: Self::TxAborts) -> Self::RxAborts {
        phase.task.enable_only_runtime_rx_aborts();
        phase.advance()
    }
    fn order_before_stop(phase: Self::RxAborts) -> Self::Ordered {
        oer_riscv::fence();
        phase.advance()
    }
    fn stop_operation(mut phase: Self::Ordered) -> Self::Stopped {
        phase.task.issue(Opcode::Stop);
        phase.advance()
    }
    fn stop_timer0(mut phase: Self::Stopped) -> Self::Timer0Stopped {
        phase.task.issue(Opcode::Timer0Stop);
        phase.advance()
    }
    fn stop_timer1(mut phase: Self::Timer0Stopped) -> Self::TimersStopped {
        phase.task.issue(Opcode::Timer1Stop);
        phase.advance()
    }
    fn order_before_discard(phase: Self::TimersStopped) -> Self::DiscardReady {
        oer_riscv::fence();
        phase.advance()
    }
    fn discard_startup_events(mut phase: Self::DiscardReady) -> Self::Discarded {
        phase.setup.mac.discard_ieee802154_startup_events();
        phase.advance()
    }
    fn finish(phase: Self::Discarded) -> Self::Output {
        oer_riscv::fence();
        (
            Ieee802154ArmedTaskOwner { task: phase.task },
            Ieee802154InterruptOwner {
                mac: phase.setup.mac,
            },
        )
    }
}

/// S31 implementation of the consuming runtime-teardown register port.
pub struct Ieee802154TeardownPort<'a, P = MacPlacement, E = EtmPlacement, B = RxInfoPlacement>(
    PhantomData<(&'a mut (), P, E, B)>,
);
/// A teardown phase retaining the task loan, IRQ owner and (after sampling) affine sample.
#[must_use = "finish teardown before returning setup ownership"]
pub struct Ieee802154Teardown<'a, P: Placement, E: Placement, B: Placement, const STEP: u8, S = ()>
{
    task: &'a mut Ieee802154TaskOwner<P, E, B>,
    interrupt: Ieee802154InterruptOwner<P>,
    sample: S,
}
impl<'a, P: Placement, E: Placement, B: Placement, const STEP: u8, S>
    Ieee802154Teardown<'a, P, E, B, STEP, S>
{
    fn advance<const NEXT: u8>(self) -> Ieee802154Teardown<'a, P, E, B, NEXT, S> {
        Ieee802154Teardown {
            task: self.task,
            interrupt: self.interrupt,
            sample: self.sample,
        }
    }
}
impl<'a, P: Placement + 'a, E: Placement + 'a, B: Placement + 'a> Ieee802154InterruptTeardown
    for Ieee802154TeardownPort<'a, P, E, B>
{
    type Task = &'a mut Ieee802154TaskOwner<P, E, B>;
    type Interrupt = Ieee802154InterruptOwner<P>;
    type Stopped = Ieee802154Teardown<'a, P, E, B, 1>;
    type Timer0Stopped = Ieee802154Teardown<'a, P, E, B, 2>;
    type TimersStopped = Ieee802154Teardown<'a, P, E, B, 3>;
    type SampleReady = Ieee802154Teardown<'a, P, E, B, 4>;
    type Sampled =
        Ieee802154Teardown<'a, P, E, B, 5, oer_esp32s31_pac::ieee802154::event_status::Snapshot>;
    type Masked =
        Ieee802154Teardown<'a, P, E, B, 6, oer_esp32s31_pac::ieee802154::event_status::Snapshot>;
    type TxMasked =
        Ieee802154Teardown<'a, P, E, B, 7, oer_esp32s31_pac::ieee802154::event_status::Snapshot>;
    type AllMasked =
        Ieee802154Teardown<'a, P, E, B, 8, oer_esp32s31_pac::ieee802154::event_status::Snapshot>;
    type Acknowledged = Ieee802154Teardown<'a, P, E, B, 9>;
    type Output = Ieee802154InterruptSetupOwner<P>;
    fn stop_operation(interrupt: Self::Interrupt, task: Self::Task) -> Self::Stopped {
        task.issue(Opcode::Stop);
        Ieee802154Teardown {
            task,
            interrupt,
            sample: (),
        }
    }
    fn stop_timer0(phase: Self::Stopped) -> Self::Timer0Stopped {
        phase.task.issue(Opcode::Timer0Stop);
        phase.advance()
    }
    fn stop_timer1(phase: Self::Timer0Stopped) -> Self::TimersStopped {
        phase.task.issue(Opcode::Timer1Stop);
        phase.advance()
    }
    fn order_before_sample(phase: Self::TimersStopped) -> Self::SampleReady {
        oer_riscv::fence();
        phase.advance()
    }
    fn sample_events(phase: Self::SampleReady) -> Self::Sampled {
        let sample = phase.interrupt.mac.sample_event_status();
        Ieee802154Teardown {
            task: phase.task,
            interrupt: phase.interrupt,
            sample,
        }
    }
    fn mask_events(phase: Self::Sampled) -> Self::Masked {
        phase.task.mask_all_events();
        phase.advance()
    }
    fn mask_tx_aborts(phase: Self::Masked) -> Self::TxMasked {
        phase.task.mask_all_tx_aborts();
        phase.advance()
    }
    fn mask_rx_aborts(phase: Self::TxMasked) -> Self::AllMasked {
        phase.task.mask_all_rx_aborts();
        phase.advance()
    }
    fn acknowledge_events(mut phase: Self::AllMasked) -> Self::Acknowledged {
        phase.interrupt.mac.acknowledge_event_status(phase.sample);
        Ieee802154Teardown {
            task: phase.task,
            interrupt: phase.interrupt,
            sample: (),
        }
    }
    fn finish(phase: Self::Acknowledged) -> Self::Output {
        oer_riscv::fence();
        Ieee802154InterruptSetupOwner {
            mac: phase.interrupt.mac,
        }
    }
}
