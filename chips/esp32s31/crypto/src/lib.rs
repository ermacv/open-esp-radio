#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Value-owned Typical AES. No DMA, AES ISR, allocation or software fallback.
//! Each block yields through an owned timer wait before sampling completion.
//! SOURCE: TRM_S31_V01_AES §10.5.3 and ESP-IDF 4d80c1b2 aes_ll.h/aes_hal.c.
//! Neither source establishes a <=20 us bound, so there is no busy settle.

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use oer_crypto::{
    KeyUnwrap, KeyUnwrapDone, KeyUnwrapRequest, KeyUnwrapResponse,
    block::{BlockError, CipherBlock},
    rfc3394::{Rfc3394Pending, Rfc3394Step, Rfc3394Unwrap},
};
use oer_esp32s31_hal::peripherals::aes::{BlockWords, IDLE, KeyWords, Registers, WORK};
use oer_time::{Clock, Duration, Instant};
use zeroize::Zeroize;

/// Scheduling policy, not a hardware latency claim: sample on timer-woken
/// 1 us steps and refuse a block still working after 1 ms.
const SAMPLE_PERIOD: Duration = Duration::from_micros(1);
const BLOCK_BUDGET: Duration = Duration::from_millis(1);

/// Constructs a movable wait without borrowing the engine. A concrete
/// factory type keeps the image's call graph static for stack analysis.
pub trait WaitFactory {
    type Wait: Future<Output = ()> + Unpin;
    fn wait_until(&self, at: Instant) -> Self::Wait;
}

impl<F, W> WaitFactory for F
where
    F: Fn(Instant) -> W,
    W: Future<Output = ()> + Unpin,
{
    type Wait = W;
    fn wait_until(&self, at: Instant) -> W {
        self(at)
    }
}

/// One client owns the engine by value; a failed engine retains its port
/// and refuses subsequent requests without touching hardware again.
pub struct AesKeyUnwrap<R: Registers, C, F> {
    registers: R,
    clock: C,
    wait: F,
    active: bool,
    poisoned: bool,
}

impl<R: Registers, C, F> AesKeyUnwrap<R, C, F> {
    pub const fn new(registers: R, clock: C, wait: F) -> Self {
        Self {
            registers,
            clock,
            wait,
            active: false,
            poisoned: false,
        }
    }

    fn clear(&mut self) {
        if self.active {
            // SOURCE: aes_ll_reset_register and the AES register reset values.
            // No DMA ever reaches request storage. Reset restores the documented
            // output state; explicit writes erase all software key/input words.
            self.registers.reset(true);
            self.registers.reset(false);
            self.registers.write_key(&KeyWords::default());
            self.registers.write_block(&BlockWords::default());
            self.registers.clock(false);
            self.active = false;
        }
    }

    fn initialize(&mut self, key: &oer_crypto::Aes128Key) -> Result<(), BlockError> {
        if self.poisoned {
            return Err(BlockError::Unavailable);
        }
        self.active = true;
        self.registers.clock(true);
        self.registers.reset(true);
        self.registers.reset(false);
        self.registers.release_parent_resets();
        self.registers.disable_interrupt();
        self.registers.disable_dma();
        self.registers.decrypt128();
        self.registers.disable_pseudo_rounds();
        self.registers.hold();
        let observation = self.registers.state();
        if observation != IDLE {
            self.poisoned = true;
            return Err(BlockError::InvalidState { observation });
        }
        let mut block = to_words(key.as_bytes());
        let mut words = KeyWords::default();
        for (out, word) in words.iter_mut().zip(&block) {
            *out = *word;
        }
        block.zeroize();
        self.registers.write_key(&words);
        words.zeroize();
        Ok(())
    }
}

impl<R: Registers, C, F> Drop for AesKeyUnwrap<R, C, F> {
    fn drop(&mut self) {
        self.clear();
    }
}

/// A movable operation: even dropping it before its first poll erases KEK;
/// dropping it during a wait resets/erases the owned block before release.
pub struct Operation<R: Registers, C, F, W, const N: usize> {
    engine: Option<AesKeyUnwrap<R, C, F>>,
    request: Option<KeyUnwrapRequest<N>>,
    step: Option<Rfc3394Step<N>>,
    pending: Option<Rfc3394Pending<N>>,
    wait: Option<W>,
    end: Instant,
}

impl<
    R: Registers + Unpin,
    C: Clock + Unpin,
    F: WaitFactory<Wait = W> + Unpin,
    W: Future<Output = ()> + Unpin,
    const N: usize,
> KeyUnwrap<N> for AesKeyUnwrap<R, C, F>
{
    type Operation = Operation<R, C, F, W, N>;
    fn unwrap_key(self, request: KeyUnwrapRequest<N>) -> Self::Operation {
        Operation {
            engine: Some(self),
            request: Some(request),
            step: None,
            pending: None,
            wait: None,
            end: Instant::EPOCH,
        }
    }
}

impl<
    R: Registers + Unpin,
    C: Clock + Unpin,
    F: WaitFactory<Wait = W> + Unpin,
    W: Future<Output = ()> + Unpin,
    const N: usize,
> Operation<R, C, F, W, N>
{
    fn finish(
        &mut self,
        response: KeyUnwrapResponse<N>,
    ) -> Poll<KeyUnwrapDone<AesKeyUnwrap<R, C, F>, N>> {
        match self.engine.take() {
            Some(mut engine) => {
                engine.clear();
                Poll::Ready(KeyUnwrapDone { engine, response })
            }
            None => Poll::Pending,
        }
    }

    /// Arm/poll a real timer once. If already elapsed, request exactly one
    /// scheduling turn; never self-wake to repeatedly sample hardware.
    fn yield_wait(&mut self, cx: &mut Context<'_>) {
        if let Some(wait) = self.wait.as_mut()
            && Pin::new(wait).poll(cx).is_ready()
        {
            self.wait = None;
            cx.waker().wake_by_ref();
        }
    }
}

impl<
    R: Registers + Unpin,
    C: Clock + Unpin,
    F: WaitFactory<Wait = W> + Unpin,
    W: Future<Output = ()> + Unpin,
    const N: usize,
> Future for Operation<R, C, F, W, N>
{
    type Output = KeyUnwrapDone<AesKeyUnwrap<R, C, F>, N>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(engine) = this.engine.as_mut() else {
            return Poll::Pending;
        };
        if let Some(request) = this.request.take() {
            let (kek, wrapped) = request.into_parts();
            this.step = Some(match Rfc3394Unwrap::start(wrapped) {
                Ok(machine) => match engine.initialize(&kek) {
                    Ok(()) => machine.next_block(),
                    Err(error) => Rfc3394Step::Complete(machine.fail(error)),
                },
                Err(response) => Rfc3394Step::Complete(response),
            });
            drop(kek);
        }
        if let Some(wait) = this.wait.as_mut() {
            if Pin::new(wait).poll(cx).is_pending() {
                return Poll::Pending;
            }
            this.wait = None;
        }
        if let Some(pending) = this.pending.take() {
            let observation = engine.registers.state();
            let result = if observation == IDLE {
                let mut words = engine.registers.read_block();
                let output = from_words(&words);
                words.zeroize();
                Ok(output)
            } else if observation == WORK && engine.clock.now() < this.end {
                let next = engine
                    .clock
                    .now()
                    .saturating_add(SAMPLE_PERIOD)
                    .min(this.end);
                this.pending = Some(pending);
                this.wait = Some(engine.wait.wait_until(next));
                this.yield_wait(cx);
                return Poll::Pending;
            } else {
                engine.poisoned = true;
                Err(if observation == WORK {
                    BlockError::Timeout { observation }
                } else {
                    BlockError::InvalidState { observation }
                })
            };
            this.step = Some(pending.complete(result));
        }
        match this.step.take() {
            Some(Rfc3394Step::Complete(response)) => this.finish(response),
            Some(Rfc3394Step::Block { input, pending }) => {
                let now = engine.clock.now();
                let (Some(end), Some(next)) = (
                    now.checked_add(BLOCK_BUDGET),
                    now.checked_add(SAMPLE_PERIOD),
                ) else {
                    return this.finish(pending.fail(BlockError::TimeOverflow));
                };
                let mut words = to_words(input.as_bytes());
                engine.registers.write_block(&words);
                words.zeroize();
                drop(input);
                engine.registers.start();
                this.end = end;
                this.pending = Some(pending);
                this.wait = Some(engine.wait.wait_until(next));
                this.yield_wait(cx);
                Poll::Pending
            }
            None => Poll::Pending,
        }
    }
}

fn to_words(bytes: &[u8; 16]) -> BlockWords {
    let [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p] = *bytes;
    [
        u32::from_le_bytes([a, b, c, d]),
        u32::from_le_bytes([e, f, g, h]),
        u32::from_le_bytes([i, j, k, l]),
        u32::from_le_bytes([m, n, o, p]),
    ]
}

fn from_words(words: &BlockWords) -> CipherBlock {
    let mut bytes = [0; 16];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        for (out, byte) in chunk.iter_mut().zip(word.to_le_bytes()) {
            *out = byte;
        }
    }
    CipherBlock::new(bytes)
}

#[cfg(test)]
mod tests;
