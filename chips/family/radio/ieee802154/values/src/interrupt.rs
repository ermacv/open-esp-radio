// SPDX-License-Identifier: Apache-2.0
//! Affine phase contracts: only the final startup fence releases a bindable IRQ.

/// Consuming startup atoms. Intermediate values retain both unbound owners.
pub trait Ieee802154InterruptStartup {
    type Task;
    type Setup;
    type Masked;
    type TxAborts;
    type RxAborts;
    type Ordered;
    type Stopped;
    type Timer0Stopped;
    type TimersStopped;
    type DiscardReady;
    type Discarded;
    type Output;
    fn mask_events(setup: Self::Setup, task: Self::Task) -> Self::Masked;
    fn runtime_tx_aborts(phase: Self::Masked) -> Self::TxAborts;
    fn runtime_rx_aborts(phase: Self::TxAborts) -> Self::RxAborts;
    fn order_before_stop(phase: Self::RxAborts) -> Self::Ordered;
    fn stop_operation(phase: Self::Ordered) -> Self::Stopped;
    fn stop_timer0(phase: Self::Stopped) -> Self::Timer0Stopped;
    fn stop_timer1(phase: Self::Timer0Stopped) -> Self::TimersStopped;
    fn order_before_discard(phase: Self::TimersStopped) -> Self::DiscardReady;
    fn discard_startup_events(phase: Self::DiscardReady) -> Self::Discarded;
    fn finish(phase: Self::Discarded) -> Self::Output;
}
/// Consuming teardown atoms. The saved runtime sample crosses the masks affinely.
pub trait Ieee802154InterruptTeardown {
    type Task;
    type Interrupt;
    type Stopped;
    type Timer0Stopped;
    type TimersStopped;
    type SampleReady;
    type Sampled;
    type Masked;
    type TxMasked;
    type AllMasked;
    type Acknowledged;
    type Output;
    fn stop_operation(interrupt: Self::Interrupt, task: Self::Task) -> Self::Stopped;
    fn stop_timer0(phase: Self::Stopped) -> Self::Timer0Stopped;
    fn stop_timer1(phase: Self::Timer0Stopped) -> Self::TimersStopped;
    fn order_before_sample(phase: Self::TimersStopped) -> Self::SampleReady;
    fn sample_events(phase: Self::SampleReady) -> Self::Sampled;
    fn mask_events(phase: Self::Sampled) -> Self::Masked;
    fn mask_tx_aborts(phase: Self::Masked) -> Self::TxMasked;
    fn mask_rx_aborts(phase: Self::TxMasked) -> Self::AllMasked;
    fn acknowledge_events(phase: Self::AllMasked) -> Self::Acknowledged;
    fn finish(phase: Self::Acknowledged) -> Self::Output;
}
