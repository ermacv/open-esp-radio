//! The receive producer's publisher into the port.
//!
//! The DMA receive transaction (`oer-esp32s31-ieee80211`'s
//! `rx::transaction::service`) hands each completed unit to a `Publisher`.
//! [`Esp32s31PortRxPublisher`] hands it to the port as an
//! [`Esp32s31StagedRx`], classified as the direct path classifies it
//! ([`IngressClass::of`]): protected data is bulk, every other frame
//! critical, so the transaction's credit reserve keeps the last staging
//! credits for management, control and EAPOL frames when ordinary data
//! fills the port. A unit the port has no room for goes back to the
//! transaction, which keeps it; nothing is lost on the way.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_esp32s31_ieee80211::rx::transaction::{
    CompletedUnit, IngressClass, IngressRoute, Preview, Publisher,
};
use oer_esp32s31_ieee80211_mac::rx::{RxError, RxIngressConfig, pool::NetworkRxFrame};

use super::Esp32s31StagedRx;

/// Where the publisher hands received units: the port's received queue.
pub trait PortRxSink<U> {
    /// Units that fit now.
    fn received_room(&self) -> usize;

    /// Hand `unit` over: queued, dropped by the port's receive rules, or
    /// back when there is no room. `Err` when its hardware report does not
    /// decode, which drops it.
    fn try_on_received(&self, unit: U) -> Result<Result<(), U>, RxError>;
}

/// The publisher of a standalone interface's receive transaction into the
/// port `sink`, whose received queue holds `DEPTH` units.
pub struct Esp32s31PortRxPublisher<'p, S, const DEPTH: usize> {
    sink: &'p S,
    config: RxIngressConfig,
    undecodable: AtomicU32,
}

impl<'p, S, const DEPTH: usize> Esp32s31PortRxPublisher<'p, S, DEPTH> {
    /// A publisher of units of the ring `config` describes into `sink`.
    pub const fn new(sink: &'p S, config: RxIngressConfig) -> Self {
        Self {
            sink,
            config,
            undecodable: AtomicU32::new(0),
        }
    }

    /// Units dropped because their hardware report did not decode.
    pub fn undecodable(&self) -> u32 {
        self.undecodable.load(Ordering::Relaxed)
    }
}

impl<'pool, S, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    Publisher<'pool, CAPACITY, SLOTS> for Esp32s31PortRxPublisher<'_, S, DEPTH>
where
    S: PortRxSink<Esp32s31StagedRx<'pool, SLOTS, CAPACITY>>,
{
    const DEPTH: usize = DEPTH;

    fn free_capacity(&self) -> usize {
        self.sink.received_room()
    }

    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
        let frame_control = Some(u16::from_le_bytes([bytes[0], bytes[1]]));
        Preview {
            unit,
            frame_control,
            class: IngressClass::of(frame_control),
            route: IngressRoute::Standalone,
        }
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        Preview {
            unit,
            frame_control: None,
            class: IngressClass::Unclassified,
            route: IngressRoute::Standalone,
        }
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        match self
            .sink
            .try_on_received(Esp32s31StagedRx::new(frame, self.config))
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(unit)) => Err(unit.into_frame()),
            Err(_) => {
                self.undecodable.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
        }
    }
}
