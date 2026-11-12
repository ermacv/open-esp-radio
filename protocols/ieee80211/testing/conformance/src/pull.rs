//! One received unit at a time from a port's receive batches, for checks
//! that look at units one by one.

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{DataPort, PortRxBuffer, PortTypes, RxThen};

/// What one [`pull`] found.
pub enum Pulled<F, M> {
    /// One received unit, the caller's.
    Unit {
        /// The frame.
        frame: F,
        /// What the hardware observed of it.
        meta: M,
    },
    /// The ring holds nothing now.
    Empty,
    /// The ring has work from this instant on.
    Later(Instant),
}

/// The pull of the port `P`.
pub type PortPulled<'id, P> =
    Pulled<PortRxBuffer<'id, P>, <<P as DataPort<'id>>::Types as PortTypes<'id>>::RxMeta>;

/// The batches a [`pull`] begins at most before it answers that work is
/// left: each one that gives no unit says the ring has more.
const BATCHES: usize = 8;

/// Take one unit from `port`'s receive batches at `now`, while the caller
/// takes at most `room`: the first unit a batch gives, or how the ring goes
/// on. The batch's other units stay with the port.
///
/// # Errors
///
/// The port's contract fault.
pub fn pull<'id, P: DataPort<'id>>(
    port: &mut P,
    room: u8,
    now: Instant,
) -> Result<PortPulled<'id, P>, P::Fault> {
    for _ in 0..BATCHES {
        let mut batch = port.begin_receive(room, now)?;
        if let Some((frame, meta)) = port.take_ready(&mut batch)? {
            return Ok(Pulled::Unit { frame, meta });
        }
        match batch.then() {
            RxThen::More => {}
            RxThen::Empty => return Ok(Pulled::Empty),
            RxThen::Later(at) => return Ok(Pulled::Later(at)),
        }
    }
    Ok(Pulled::Later(now))
}
