//! The image's handlers, as the console calls them.
//!
//! An image gives the console two values, split once at its composition:
//! what **observes** (its synchronous handlers, answered in the step that
//! decoded the request) and what **commands** (its asynchronous ones, awaited
//! by the console's task beside its read of the endpoint). A command's future
//! borrows the commanding value alone, so an observation is answered while a
//! command works; one command is in work at a time by that borrow, and a
//! second is refused busy. Every answer is serialized once, from a reference,
//! into the console's frame ([`Frame::write`]).

use core::convert::Infallible;
use core::future::Future;

use oer_protocols_hil_protocol::{Addressed, EncodeError, FrameEncoder, Message, RequestIdentity};

use crate::base::{Either, NoRequests, Requests};

/// A trap on an illegal instruction after an answer's write: what only a
/// service set that declares it (the fault service) may ask for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Trap;

/// What a service set may ask after an answer's write: [`Trap`], or
/// `Infallible` for a set that never traps. Two sets together trap when
/// either does.
pub trait Traps: Sized {
    type Or<B: Traps>: Traps;
    /// This set's trap, as the trap of this set beside `B`.
    fn left<B: Traps>(self) -> Self::Or<B>;
    /// `B`'s trap, as the trap of this set beside `B`.
    fn right<B: Traps>(other: B) -> Self::Or<B>;
}

impl Traps for Infallible {
    type Or<B: Traps> = B;
    fn left<B: Traps>(self) -> B {
        match self {}
    }
    fn right<B: Traps>(other: B) -> B {
        other
    }
}

impl Traps for Trap {
    type Or<B: Traps> = Trap;
    fn left<B: Traps>(self) -> Trap {
        self
    }
    fn right<B: Traps>(_other: B) -> Trap {
        Trap
    }
}

/// What the console does once it wrote an answer whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum After<T> {
    Nothing,
    /// End the console's run with this trap.
    Trap(T),
}

impl<T: Traps> After<T> {
    fn left<B: Traps>(self) -> After<T::Or<B>> {
        match self {
            Self::Nothing => After::Nothing,
            Self::Trap(trap) => After::Trap(trap.left()),
        }
    }

    fn right<A: Traps>(self) -> After<A::Or<T>> {
        match self {
            Self::Nothing => After::Nothing,
            Self::Trap(trap) => After::Trap(A::right(trap)),
        }
    }
}

/// Where one answer is written: the console's encoder, and the frame's
/// address, numbered when it is written.
pub struct Frame<'f> {
    encoder: &'f mut FrameEncoder,
    sequence: &'f mut u32,
    boot: u64,
    request: RequestIdentity,
}

impl<'f> Frame<'f> {
    pub(crate) const fn new(
        encoder: &'f mut FrameEncoder,
        sequence: &'f mut u32,
        boot: u64,
        request: RequestIdentity,
    ) -> Self {
        Self {
            encoder,
            sequence,
            boot,
            request,
        }
    }

    /// Serializes `body` from its reference as the answer: the frame's
    /// bytes, in the encoder's buffer.
    pub fn write<M: Message>(self, body: &M) -> Written<'f> {
        let to = Addressed {
            boot_id: self.boot,
            message_sequence: *self.sequence,
            session_id: self.request.session_id,
            request_id: self.request.request_id,
        };
        *self.sequence = self.sequence.wrapping_add(1);
        Written(self.encoder.encode_message(to, body))
    }
}

/// An answer's frame, encoded, or why it could not be.
pub struct Written<'f>(pub(crate) Result<&'f [u8], EncodeError>);

/// What a request of the image became in the console's step.
pub enum Served<'f, C> {
    /// An observation, answered: its frame.
    Observed(Written<'f>),
    /// A command, to run on the commanding value.
    Command(C),
}

/// A command's answer, written from where it lies.
pub trait Answer {
    fn write<'f>(&self, frame: Frame<'f>) -> Written<'f>;
}

impl Answer for Infallible {
    fn write<'f>(&self, _frame: Frame<'f>) -> Written<'f> {
        match *self {}
    }
}

impl<A: Answer, B: Answer> Answer for Either<A, B> {
    fn write<'f>(&self, frame: Frame<'f>) -> Written<'f> {
        match self {
            Self::First(answer) => answer.write(frame),
            Self::Second(answer) => answer.write(frame),
        }
    }
}

/// A command that gave no answer: its owner's level did not give one. The
/// console refuses it to the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unanswered;

/// What a command gives: its answer and what follows its write.
pub type Ran<A, T> = Result<(A, After<T>), Unanswered>;

/// A service set's requests, served on the observing value `O`.
pub trait Serve<O>: Requests {
    /// The set's commands.
    type Command;
    /// Answers an observation in `frame`, or gives the command.
    fn serve<'f>(self, observer: &mut O, frame: Frame<'f>) -> Served<'f, Self::Command>;
}

/// A command, run on the commanding value `C`.
pub trait Run<C> {
    type Answer: Answer;
    type Trap: Traps;
    fn run(self, commander: &mut C) -> impl Future<Output = Ran<Self::Answer, Self::Trap>>;
}

impl<O> Serve<O> for NoRequests {
    type Command = Infallible;
    fn serve<'f>(self, _observer: &mut O, _frame: Frame<'f>) -> Served<'f, Infallible> {
        match self {}
    }
}

impl<C> Run<C> for Infallible {
    type Answer = Infallible;
    type Trap = Infallible;
    async fn run(self, _commander: &mut C) -> Ran<Infallible, Infallible> {
        match self {}
    }
}

impl<O1, O2, A: Serve<O1>, B: Serve<O2>> Serve<(O1, O2)> for Either<A, B> {
    type Command = Either<A::Command, B::Command>;
    fn serve<'f>(self, observer: &mut (O1, O2), frame: Frame<'f>) -> Served<'f, Self::Command> {
        match self {
            Self::First(request) => match request.serve(&mut observer.0, frame) {
                Served::Observed(written) => Served::Observed(written),
                Served::Command(command) => Served::Command(Either::First(command)),
            },
            Self::Second(request) => match request.serve(&mut observer.1, frame) {
                Served::Observed(written) => Served::Observed(written),
                Served::Command(command) => Served::Command(Either::Second(command)),
            },
        }
    }
}

impl<C1, C2, A: Run<C1>, B: Run<C2>> Run<(C1, C2)> for Either<A, B> {
    type Answer = Either<A::Answer, B::Answer>;
    type Trap = <A::Trap as Traps>::Or<B::Trap>;
    async fn run(self, commander: &mut (C1, C2)) -> Ran<Self::Answer, Self::Trap> {
        match self {
            Self::First(command) => {
                let (answer, after) = command.run(&mut commander.0).await?;
                Ok((Either::First(answer), after.left()))
            }
            Self::Second(command) => {
                let (answer, after) = command.run(&mut commander.1).await?;
                Ok((Either::Second(answer), after.right::<A::Trap>()))
            }
        }
    }
}
