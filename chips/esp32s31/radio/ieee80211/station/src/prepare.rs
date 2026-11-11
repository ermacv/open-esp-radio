//! The staged preparation of a port's parts before its radio starts: the
//! receive path's bind, the device's place, each transmit descriptor's pin
//! into its slot, each aggregate chain, the lent units' notifies and the
//! shared storage's place, in that order. A refused step returns what it
//! holds of its own input (the input whole, or the owning intermediate it
//! reached: a placement split, a place claimed, a storage pinned); a
//! refusal returns the step it refused at, that step's own, every part the
//! steps before it reached and every input no step took yet. Generic over the steps ([`Steps`]): the
//! chip's install names its own; a host test names its own and refuses at
//! each step through this same path.

/// The transmit descriptors a port prepares: its transmitter's first one,
/// then one per EDCA queue.
pub(crate) const SLOTS: usize = 5;
/// The aggregate chains a port prepares.
pub(crate) const CHAINS: usize = 2;

/// One kind of step of a port's preparation, each with its input, the
/// part it makes and what its refusal returns of its input (`*Back`).
pub trait Steps {
    /// Why a step refuses.
    type Error;
    type RxIn;
    type Rx;
    type RxBack;
    type DeviceIn;
    type Device;
    type DeviceBack;
    type SlotIn;
    type Slot;
    type SlotBack;
    type ChainIn;
    type Chain;
    type ChainBack;
    type LentIn;
    type Lent;
    type LentBack;
    type SharedIn;
    type Shared;
    type SharedBack;
    /// Bind the receive path.
    fn bind(&mut self, input: Self::RxIn) -> StepResult<Self::Rx, Self::Error, Self::RxBack>;
    /// Take the device's place.
    fn device(
        &mut self,
        input: Self::DeviceIn,
    ) -> StepResult<Self::Device, Self::Error, Self::DeviceBack>;
    /// Pin one transmit descriptor into its slot.
    fn slot(&mut self, input: Self::SlotIn) -> StepResult<Self::Slot, Self::Error, Self::SlotBack>;
    /// Make one aggregate chain.
    fn chain(
        &mut self,
        input: Self::ChainIn,
    ) -> StepResult<Self::Chain, Self::Error, Self::ChainBack>;
    /// Take the lent units' notifies.
    fn lent(&mut self, input: Self::LentIn) -> StepResult<Self::Lent, Self::Error, Self::LentBack>;
    /// Take the shared storage's place.
    fn shared(
        &mut self,
        input: Self::SharedIn,
    ) -> StepResult<Self::Shared, Self::Error, Self::SharedBack>;
}

/// Why one step refused: its cause, and what it holds of its input (`B`:
/// the input whole, or the owning intermediate its primitive reached).
pub struct StepRefused<E, B> {
    pub cause: E,
    pub back: B,
}

/// What one step gives: its part, or its refusal.
pub type StepResult<O, E, B> = Result<O, StepRefused<E, B>>;

/// The step a preparation refused at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Bind,
    Device,
    /// The transmit descriptor at this index ([`SLOTS`]).
    Slot(usize),
    /// The aggregate chain at this index ([`CHAINS`]).
    Chain(usize),
    Lent,
    Shared,
}

/// Every input of a preparation.
pub(crate) struct Inputs<S: Steps> {
    pub rx: S::RxIn,
    pub device: S::DeviceIn,
    pub slots: [S::SlotIn; SLOTS],
    pub chains: [S::ChainIn; CHAINS],
    pub lent: S::LentIn,
    pub shared: S::SharedIn,
}

/// Every part of a completed preparation.
pub(crate) struct Parts<S: Steps> {
    pub rx: S::Rx,
    pub device: S::Device,
    pub slots: [S::Slot; SLOTS],
    pub chains: [S::Chain; CHAINS],
    pub lent: S::Lent,
    pub shared: S::Shared,
}

/// A slot of a refused preparation: each holds what its step had when the
/// preparation refused, or nothing.
pub struct Stage<Rx, Device, Slot, Chain, Lent, Shared> {
    pub rx: Option<Rx>,
    pub device: Option<Device>,
    pub slots: [Option<Slot>; SLOTS],
    pub chains: [Option<Chain>; CHAINS],
    pub lent: Option<Lent>,
    pub shared: Option<Shared>,
}

impl<Rx, Device, Slot, Chain, Lent, Shared> Stage<Rx, Device, Slot, Chain, Lent, Shared> {
    const fn none() -> Self {
        Self {
            rx: None,
            device: None,
            slots: [const { None }; SLOTS],
            chains: [const { None }; CHAINS],
            lent: None,
            shared: None,
        }
    }
}

/// The parts a refused preparation reached.
pub type ReachedOf<S> = Stage<
    <S as Steps>::Rx,
    <S as Steps>::Device,
    <S as Steps>::Slot,
    <S as Steps>::Chain,
    <S as Steps>::Lent,
    <S as Steps>::Shared,
>;
/// What the refused step of a preparation returned of its input: only that
/// step's place holds it.
pub type BackOf<S> = Stage<
    <S as Steps>::RxBack,
    <S as Steps>::DeviceBack,
    <S as Steps>::SlotBack,
    <S as Steps>::ChainBack,
    <S as Steps>::LentBack,
    <S as Steps>::SharedBack,
>;
/// The inputs no step of a refused preparation took.
pub type RemainingOf<S> = Stage<
    <S as Steps>::RxIn,
    <S as Steps>::DeviceIn,
    <S as Steps>::SlotIn,
    <S as Steps>::ChainIn,
    <S as Steps>::LentIn,
    <S as Steps>::SharedIn,
>;

/// A refused preparation: why, at which step, what that step returned of
/// its input, the parts the steps before it reached and the inputs no step
/// took.
#[must_use = "a refused preparation returns the parts it reached and the inputs it did not take"]
pub struct PreparationRefused<S: Steps> {
    pub cause: S::Error,
    pub step: Step,
    pub back: BackOf<S>,
    pub reached: ReachedOf<S>,
    pub remaining: RemainingOf<S>,
}

/// Run every step of the preparation, in order.
///
/// # Errors
///
/// [`PreparationRefused`]: the first step that refused, with every part reached and
/// every input left.
#[allow(
    clippy::result_large_err,
    reason = "a refused preparation returns each part and input by value"
)]
pub(crate) fn prepare<S: Steps>(
    steps: &mut S,
    inputs: Inputs<S>,
) -> Result<Parts<S>, PreparationRefused<S>> {
    let Inputs {
        rx,
        device,
        slots,
        chains,
        lent,
        shared,
    } = inputs;
    let [s0, s1, s2, s3, s4] = slots;
    let [c0, c1] = chains;
    // Run one step; on its refusal, the parts named reached and the inputs
    // named remaining go out.
    macro_rules! step {
        ($call:expr, $step:expr; back $b:ident $([$bi:literal])?;
         reached [$($r:ident $([$ri:literal])? => $rv:expr),*];
         remaining [$($m:ident $([$mi:literal])? => $mv:expr),*]) => {
            match $call {
                Ok(part) => part,
                Err(StepRefused { cause, back: returned }) => {
                    #[allow(unused_mut, reason = "a step with nothing before it reaches nothing")]
                    let mut reached = ReachedOf::<S>::none();
                    $( reached.$r $([$ri])? = Some($rv); )*
                    #[allow(unused_mut, reason = "the last step leaves no input")]
                    let mut remaining = RemainingOf::<S>::none();
                    $( remaining.$m $([$mi])? = Some($mv); )*
                    // What the refused step returned of its input.
                    let mut back = BackOf::<S>::none();
                    back.$b $([$bi])? = Some(returned);
                    return Err(PreparationRefused { cause, step: $step, back, reached, remaining });
                }
            }
        };
    }
    let rx = step!(steps.bind(rx), Step::Bind; back rx;
        reached [];
        remaining [device => device, slots[0] => s0, slots[1] => s1, slots[2] => s2,
            slots[3] => s3, slots[4] => s4, chains[0] => c0, chains[1] => c1,
            lent => lent, shared => shared]);
    let device = step!(steps.device(device), Step::Device; back device;
        reached [rx => rx];
        remaining [slots[0] => s0, slots[1] => s1, slots[2] => s2, slots[3] => s3,
            slots[4] => s4, chains[0] => c0, chains[1] => c1, lent => lent, shared => shared]);
    let p0 = step!(steps.slot(s0), Step::Slot(0); back slots[0];
        reached [rx => rx, device => device];
        remaining [slots[1] => s1, slots[2] => s2, slots[3] => s3, slots[4] => s4,
            chains[0] => c0, chains[1] => c1, lent => lent, shared => shared]);
    let p1 = step!(steps.slot(s1), Step::Slot(1); back slots[1];
        reached [rx => rx, device => device, slots[0] => p0];
        remaining [slots[2] => s2, slots[3] => s3, slots[4] => s4,
            chains[0] => c0, chains[1] => c1, lent => lent, shared => shared]);
    let p2 = step!(steps.slot(s2), Step::Slot(2); back slots[2];
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1];
        remaining [slots[3] => s3, slots[4] => s4,
            chains[0] => c0, chains[1] => c1, lent => lent, shared => shared]);
    let p3 = step!(steps.slot(s3), Step::Slot(3); back slots[3];
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1, slots[2] => p2];
        remaining [slots[4] => s4, chains[0] => c0, chains[1] => c1, lent => lent,
            shared => shared]);
    let p4 = step!(steps.slot(s4), Step::Slot(4); back slots[4];
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1, slots[2] => p2,
            slots[3] => p3];
        remaining [chains[0] => c0, chains[1] => c1, lent => lent, shared => shared]);
    let k0 = step!(steps.chain(c0), Step::Chain(0); back chains[0];
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1, slots[2] => p2,
            slots[3] => p3, slots[4] => p4];
        remaining [chains[1] => c1, lent => lent, shared => shared]);
    let k1 = step!(steps.chain(c1), Step::Chain(1); back chains[1];
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1, slots[2] => p2,
            slots[3] => p3, slots[4] => p4, chains[0] => k0];
        remaining [lent => lent, shared => shared]);
    let lent = step!(steps.lent(lent), Step::Lent; back lent;
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1, slots[2] => p2,
            slots[3] => p3, slots[4] => p4, chains[0] => k0, chains[1] => k1];
        remaining [shared => shared]);
    let shared = step!(steps.shared(shared), Step::Shared; back shared;
        reached [rx => rx, device => device, slots[0] => p0, slots[1] => p1, slots[2] => p2,
            slots[3] => p3, slots[4] => p4, chains[0] => k0, chains[1] => k1, lent => lent];
        remaining []);
    Ok(Parts {
        rx,
        device,
        slots: [p0, p1, p2, p3, p4],
        chains: [k0, k1],
        lent,
        shared,
    })
}

#[cfg(test)]
mod tests;
