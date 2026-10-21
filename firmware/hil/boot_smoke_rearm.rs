//! Shared S31/C5 alarm checks, included by both smoke images without a new crate.

use core::{future::Future, future::poll_fn, pin::pin, task::Poll};

use embassy_time::{Duration, Timer};
use esp_hal::time::Instant;

pub(crate) struct Measurement {
    pub step: u8,
    pub delay_ms: u64,
    pub elapsed_us: u64,
}

async fn elapsed_after(delay_ms: u64) -> u64 {
    let start = Instant::now();
    Timer::after(Duration::from_millis(delay_ms)).await;
    start.elapsed().as_micros()
}

fn check(
    step: u8,
    delay_ms: u64,
    elapsed_us: u64,
    report: &mut impl FnMut(&Measurement),
) -> Result<(), Measurement> {
    let measurement = Measurement {
        step,
        delay_ms,
        elapsed_us,
    };
    report(&measurement);
    if (delay_ms * 1_000..=delay_ms * 1_000 + 5_000).contains(&elapsed_us) {
        Ok(())
    } else {
        Err(measurement)
    }
}

/// Check successive alarms, an earlier deadline and cancellation before PASS.
pub(crate) async fn run(mut report: impl FnMut(&Measurement)) -> Result<(), Measurement> {
    for delay_ms in 1..=5 {
        let elapsed_us = elapsed_after(delay_ms).await;
        check(1, delay_ms, elapsed_us, &mut report)?;
    }

    // Join: poll the long timer first so its 100 ms deadline is installed
    // before the short timer asks the driver to program an earlier deadline.
    let mut long = pin!(elapsed_after(100));
    let mut short = pin!(elapsed_after(10));
    let mut long_elapsed = None;
    let mut short_elapsed = None;
    let (long_elapsed, short_elapsed) = poll_fn(|cx| {
        if long_elapsed.is_none()
            && let Poll::Ready(elapsed) = long.as_mut().poll(cx)
        {
            long_elapsed = Some(elapsed);
        }
        if short_elapsed.is_none()
            && let Poll::Ready(elapsed) = short.as_mut().poll(cx)
        {
            short_elapsed = Some(elapsed);
        }
        match (long_elapsed, short_elapsed) {
            (Some(long), Some(short)) => Poll::Ready((long, short)),
            _ => Poll::Pending,
        }
    })
    .await;
    check(2, 10, short_elapsed, &mut report)?;
    check(2, 100, long_elapsed, &mut report)?;

    // Select: both futures are dropped at the end of this scope, cancelling
    // the losing 100 ms timer before the next 20 ms timer is registered.
    let (short_won, elapsed_us) = {
        let mut long = pin!(elapsed_after(100));
        let mut short = pin!(elapsed_after(10));
        poll_fn(|cx| {
            if let Poll::Ready(elapsed) = long.as_mut().poll(cx) {
                return Poll::Ready((false, elapsed));
            }
            if let Poll::Ready(elapsed) = short.as_mut().poll(cx) {
                return Poll::Ready((true, elapsed));
            }
            Poll::Pending
        })
        .await
    };
    check(3, 10, elapsed_us, &mut report)?;
    if !short_won {
        return Err(Measurement {
            step: 3,
            delay_ms: 10,
            elapsed_us,
        });
    }
    let elapsed_us = elapsed_after(20).await;
    check(3, 20, elapsed_us, &mut report)
}
