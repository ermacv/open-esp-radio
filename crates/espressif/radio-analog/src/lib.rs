//! Chip-neutral analog register bus of the Espressif PHY.
//!
//! The PHY reaches its analog blocks (PLLs, bias, regulators, the RF front
//! end) through byte registers behind an analog I2C master. The vendor leaves
//! (`phy_chip_i2c_readReg`, `phy_chip_i2c_writeReg`) busy-wait for each
//! command; this crate splits every command into a start and an observed
//! completion, so an outer owner decides how to wait.
//!
//! The vendor transactions are transitions: [`FieldReadTransition`],
//! [`FieldWriteTransition`], [`ConfigurationTransition`] and
//! [`ParallelTransition`] name the next whole-byte read or write, or the host
//! map step, and accept its completion, without touching the bus. A chip
//! implements [`AnalogRegisterBus`] over its PAC. Its address type is opaque:
//! host selection, read masks and block aliases stay in the chip
//! implementation, and every completion is observed with the same address
//! that started the command. [`Driver`] runs a transition over that bus;
//! [`FieldRead`], [`FieldWrite`], [`Configuration`] and [`ParallelWrites`] are
//! the polled forms, and a chip may drive the transitions from its own
//! executor instead.
//!
//! This crate performs no MMIO and holds no delay or deadline; those belong
//! to the executor that polls it.

#![no_std]
#![forbid(unsafe_code)]

/// The host of an address is still executing a command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Busy;

/// Non-blocking access to one chip's analog byte registers.
///
/// Implementations must observe completion on the host that `address`
/// selected when the command started, and must not start a command while that
/// host is busy.
pub trait AnalogRegisterBus {
    /// Validated identity of one analog byte register.
    type Address: Copy + Eq + core::fmt::Debug;

    /// Start a read of `address`.
    ///
    /// # Errors
    ///
    /// The address's host is busy; no command was started.
    fn try_start_read(&mut self, address: Self::Address) -> Result<(), Busy>;

    /// The byte of the completed read of `address`.
    ///
    /// # Errors
    ///
    /// The read is still executing.
    fn try_finish_read(&self, address: Self::Address) -> Result<u8, Busy>;

    /// Start a write of `value` to `address`.
    ///
    /// # Errors
    ///
    /// The address's host is busy; no command was started.
    fn try_start_write(&mut self, address: Self::Address, value: u8) -> Result<(), Busy>;

    /// Observe the completion of the write of `address`.
    ///
    /// # Errors
    ///
    /// The write is still executing.
    fn try_finish_write(&self, address: Self::Address) -> Result<(), Busy>;
}

/// Bits `msb..=lsb` of one analog byte register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalogField<Address> {
    address: Address,
    msb: u8,
    lsb: u8,
}

impl<Address: Copy> AnalogField<Address> {
    /// The field, when `lsb <= msb <= 7`.
    pub const fn new(address: Address, msb: u8, lsb: u8) -> Option<Self> {
        if lsb <= msb && msb < 8 {
            Some(Self { address, msb, lsb })
        } else {
            None
        }
    }

    pub const fn address(self) -> Address {
        self.address
    }

    /// The field's bits in place.
    const fn mask(self) -> u8 {
        let width = self.msb - self.lsb + 1;
        ((((1_u16) << width) - 1) as u8) << self.lsb
    }

    /// The field's value in `byte`, as `phy_i2c_readReg_Mask` extracts it.
    pub const fn extract(self, byte: u8) -> u8 {
        (byte & self.mask()) >> self.lsb
    }

    /// `byte` with the field replaced, as `phy_i2c_writeReg_Mask` composes
    /// it, when `value` fits the field; `None` otherwise.
    ///
    /// The vendor leaf ORs `value << lsb` in unmasked, so a wider value
    /// would also set bits above the field; such values are rejected here.
    pub const fn insert(self, byte: u8, value: u8) -> Option<u8> {
        let width = self.msb - self.lsb + 1;
        if width < 8 && value >> width != 0 {
            return None;
        }
        Some((byte & !self.mask()) | (value << self.lsb))
    }
}

/// Progress of a polled transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step<T> {
    /// A command is executing or waits for its host; poll again.
    Pending,
    /// The transaction completed with this result.
    Ready(T),
}

/// The next whole-byte transaction a transition needs, or its result.
///
/// A transition never touches the bus: an executor performs each byte
/// transaction and reports it back with the matching [`Completion`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action<Address, T> {
    /// Read the byte at `address`.
    Read { address: Address },
    /// Write `value` to `address`.
    Write { address: Address, value: u8 },
    /// The transition completed with this result.
    Complete(T),
}

/// A completed whole-byte transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion<Address> {
    /// The read of `address` returned `value`.
    Read { address: Address, value: u8 },
    /// The write of `address` completed.
    Written { address: Address },
}

/// A completion a transition did not ask for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    /// The completion does not answer the pending action.
    WrongCompletion,
    /// The transition already completed.
    AlreadyComplete,
}

/// A transaction as a sequence of whole-byte reads and writes.
pub trait ByteTransition {
    /// The analog register identity the transition addresses.
    type Address: Copy + Eq;
    /// The result of the completed transition.
    type Output: Copy;

    /// The next byte transaction, or the result.
    fn action(&self) -> Action<Self::Address, Self::Output>;

    /// Accept the completion of the pending byte transaction.
    ///
    /// # Errors
    ///
    /// The completion does not answer the pending action; the transition is
    /// unchanged.
    fn advance(&mut self, completion: Completion<Self::Address>) -> Result<(), TransitionError>;
}

/// `phy_i2c_readReg_Mask`: read the register, extract the field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldReadTransition<Address> {
    field: AnalogField<Address>,
    value: Option<u8>,
}

impl<Address: Copy> FieldReadTransition<Address> {
    pub const fn new(field: AnalogField<Address>) -> Self {
        Self { field, value: None }
    }
}

impl<Address: Copy + Eq> ByteTransition for FieldReadTransition<Address> {
    type Address = Address;
    type Output = u8;

    fn action(&self) -> Action<Address, u8> {
        match self.value {
            None => Action::Read {
                address: self.field.address,
            },
            Some(value) => Action::Complete(value),
        }
    }

    fn advance(&mut self, completion: Completion<Address>) -> Result<(), TransitionError> {
        match (self.value, completion) {
            (Some(_), _) => Err(TransitionError::AlreadyComplete),
            (None, Completion::Read { address, value }) if address == self.field.address => {
                self.value = Some(self.field.extract(value));
                Ok(())
            }
            (None, _) => Err(TransitionError::WrongCompletion),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FieldWriteStep {
    Read,
    Write(u8),
    Complete,
}

/// Why a field write was not constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueTooWide;

/// `phy_i2c_writeReg_Mask`: read the register, replace the field and write
/// the byte back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldWriteTransition<Address> {
    field: AnalogField<Address>,
    value: u8,
    step: FieldWriteStep,
}

impl<Address: Copy> FieldWriteTransition<Address> {
    /// A write of `value` to `field`.
    ///
    /// # Errors
    ///
    /// `value` does not fit the field.
    pub const fn new(field: AnalogField<Address>, value: u8) -> Result<Self, ValueTooWide> {
        if field.insert(0, value).is_none() {
            return Err(ValueTooWide);
        }
        Ok(Self {
            field,
            value,
            step: FieldWriteStep::Read,
        })
    }

    /// The written field.
    pub const fn field(&self) -> AnalogField<Address> {
        self.field
    }

    /// The next byte transaction, or completion.
    pub const fn action(&self) -> Action<Address, ()> {
        let address = self.field.address;
        match self.step {
            FieldWriteStep::Read => Action::Read { address },
            FieldWriteStep::Write(value) => Action::Write { address, value },
            FieldWriteStep::Complete => Action::Complete(()),
        }
    }
}

impl<Address: Copy + Eq> FieldWriteTransition<Address> {
    /// Accept the completion of the pending byte transaction.
    ///
    /// # Errors
    ///
    /// The completion does not answer the pending action; the transition is
    /// unchanged.
    pub fn advance(&mut self, completion: Completion<Address>) -> Result<(), TransitionError> {
        let address = self.field.address;
        self.step = match (self.step, completion) {
            (
                FieldWriteStep::Read,
                Completion::Read {
                    address: read,
                    value,
                },
            ) if read == address => match self.field.insert(value, self.value) {
                Some(byte) => FieldWriteStep::Write(byte),
                None => unreachable!("the value was checked at construction"),
            },
            (FieldWriteStep::Write(_), Completion::Written { address: written })
                if written == address =>
            {
                FieldWriteStep::Complete
            }
            (FieldWriteStep::Complete, _) => return Err(TransitionError::AlreadyComplete),
            _ => return Err(TransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl<Address: Copy + Eq> ByteTransition for FieldWriteTransition<Address> {
    type Address = Address;
    type Output = ();

    fn action(&self) -> Action<Address, ()> {
        FieldWriteTransition::action(self)
    }

    fn advance(&mut self, completion: Completion<Address>) -> Result<(), TransitionError> {
        FieldWriteTransition::advance(self, completion)
    }
}

/// One command of a vendor analog configuration leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationCommand<Address> {
    /// `phy_i2c_readReg` whose byte the leaf discards.
    Read(Address),
    /// `phy_i2c_writeReg`: write a whole byte.
    Write(Address, u8),
    /// `phy_i2c_writeReg_Mask`: replace one field, which the value fits.
    Modify(AnalogField<Address>, u8),
}

/// A command of a configuration does not fit its field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCommand {
    pub index: usize,
}

/// The commands of one configuration leaf, by index up to the first `None`.
pub trait ConfigurationCommands {
    /// The analog register identity of the commands.
    type Address: Copy;

    /// Command `index`, or `None` past the last.
    fn command(&self, index: usize) -> Option<ConfigurationCommand<Self::Address>>;
}

impl<Address: Copy, F> ConfigurationCommands for F
where
    F: Fn(usize) -> Option<ConfigurationCommand<Address>>,
{
    type Address = Address;

    fn command(&self, index: usize) -> Option<ConfigurationCommand<Address>> {
        self(index)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigurationStep<Address> {
    Reading(usize, Address),
    Writing(usize, Address, u8),
    Modifying(usize, FieldWriteTransition<Address>),
    Invalid(usize),
    Complete,
}

/// Vendor configuration leaf: the commands `commands(0)`, `commands(1)`, ...
/// up to the first `None`, each completed before the next starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigurationTransition<Commands, Address> {
    commands: Commands,
    step: ConfigurationStep<Address>,
}

impl<Commands, Address: Copy> ConfigurationTransition<Commands, Address>
where
    Commands: ConfigurationCommands<Address = Address>,
{
    pub fn new(commands: Commands) -> Self {
        let mut transition = Self {
            commands,
            step: ConfigurationStep::Complete,
        };
        transition.step = transition.enter(0);
        transition
    }

    fn enter(&self, index: usize) -> ConfigurationStep<Address> {
        match self.commands.command(index) {
            None => ConfigurationStep::Complete,
            Some(ConfigurationCommand::Read(address)) => ConfigurationStep::Reading(index, address),
            Some(ConfigurationCommand::Write(address, value)) => {
                ConfigurationStep::Writing(index, address, value)
            }
            Some(ConfigurationCommand::Modify(field, value)) => {
                match FieldWriteTransition::new(field, value) {
                    Ok(write) => ConfigurationStep::Modifying(index, write),
                    Err(ValueTooWide) => ConfigurationStep::Invalid(index),
                }
            }
        }
    }

    /// The command whose value does not fit its field, before any bus
    /// action for it.
    pub fn invalid(&self) -> Option<InvalidCommand> {
        match self.step {
            ConfigurationStep::Invalid(index) => Some(InvalidCommand { index }),
            _ => None,
        }
    }
}

impl<Commands, Address: Copy + Eq> ByteTransition for ConfigurationTransition<Commands, Address>
where
    Commands: ConfigurationCommands<Address = Address>,
{
    type Address = Address;
    type Output = Result<(), InvalidCommand>;

    fn action(&self) -> Action<Address, Result<(), InvalidCommand>> {
        match self.step {
            ConfigurationStep::Reading(_, address) => Action::Read { address },
            ConfigurationStep::Writing(_, address, value) => Action::Write { address, value },
            ConfigurationStep::Modifying(_, write) => match write.action() {
                Action::Read { address } => Action::Read { address },
                Action::Write { address, value } => Action::Write { address, value },
                Action::Complete(()) => unreachable!("a completed field write advances"),
            },
            ConfigurationStep::Invalid(index) => Action::Complete(Err(InvalidCommand { index })),
            ConfigurationStep::Complete => Action::Complete(Ok(())),
        }
    }

    fn advance(&mut self, completion: Completion<Address>) -> Result<(), TransitionError> {
        self.step = match (self.step, completion) {
            (
                ConfigurationStep::Reading(index, address),
                Completion::Read { address: read, .. },
            ) if read == address => self.enter(index + 1),
            (
                ConfigurationStep::Writing(index, address, _),
                Completion::Written { address: written },
            ) if written == address => self.enter(index + 1),
            (ConfigurationStep::Modifying(index, mut write), completion) => {
                write.advance(completion)?;
                if write.action() == Action::Complete(()) {
                    self.enter(index + 1)
                } else {
                    ConfigurationStep::Modifying(index, write)
                }
            }
            (ConfigurationStep::Complete | ConfigurationStep::Invalid(_), _) => {
                return Err(TransitionError::AlreadyComplete);
            }
            _ => return Err(TransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BusPhase {
    Idle,
    Started,
}

/// Run a [`ByteTransition`] over an [`AnalogRegisterBus`], at most one bus
/// action per [`poll`](Self::poll): start the pending byte transaction when
/// its host is idle, then observe its completion.
#[must_use = "a transition does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct Driver<Transition> {
    transition: Transition,
    phase: BusPhase,
}

impl<Transition: ByteTransition> Driver<Transition> {
    pub const fn new(transition: Transition) -> Self {
        Self {
            transition,
            phase: BusPhase::Idle,
        }
    }

    /// Advance the transition by at most one bus action.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Step<Transition::Output>
    where
        Bus: AnalogRegisterBus<Address = Transition::Address>,
    {
        let action = self.transition.action();
        let completion = match (self.phase, action) {
            (_, Action::Complete(output)) => return Step::Ready(output),
            (BusPhase::Idle, Action::Read { address }) => {
                if bus.try_start_read(address).is_ok() {
                    self.phase = BusPhase::Started;
                }
                return Step::Pending;
            }
            (BusPhase::Idle, Action::Write { address, value }) => {
                if bus.try_start_write(address, value).is_ok() {
                    self.phase = BusPhase::Started;
                }
                return Step::Pending;
            }
            (BusPhase::Started, Action::Read { address }) => match bus.try_finish_read(address) {
                Ok(value) => Completion::Read { address, value },
                Err(Busy) => return Step::Pending,
            },
            (BusPhase::Started, Action::Write { address, .. }) => {
                match bus.try_finish_write(address) {
                    Ok(()) => Completion::Written { address },
                    Err(Busy) => return Step::Pending,
                }
            }
        };
        self.phase = BusPhase::Idle;
        match self.transition.advance(completion) {
            Ok(()) => {}
            Err(_) => unreachable!("the driver completes the action it started"),
        }
        match self.transition.action() {
            Action::Complete(output) => Step::Ready(output),
            _ => Step::Pending,
        }
    }
}

/// Polled `phy_i2c_readReg_Mask` over a bus.
#[must_use = "a field read does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct FieldRead<Address>(Driver<FieldReadTransition<Address>>);

impl<Address: Copy + Eq> FieldRead<Address> {
    pub const fn new(field: AnalogField<Address>) -> Self {
        Self(Driver::new(FieldReadTransition::new(field)))
    }

    /// Advance the read by at most one bus action.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Step<u8>
    where
        Bus: AnalogRegisterBus<Address = Address>,
    {
        self.0.poll(bus)
    }
}

/// Polled `phy_i2c_writeReg_Mask` over a bus.
#[must_use = "a field write does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct FieldWrite<Address>(Driver<FieldWriteTransition<Address>>);

impl<Address: Copy + Eq> FieldWrite<Address> {
    /// A write of `value` to `field`.
    ///
    /// # Errors
    ///
    /// `value` does not fit the field.
    pub const fn new(field: AnalogField<Address>, value: u8) -> Result<Self, ValueTooWide> {
        match FieldWriteTransition::new(field, value) {
            Ok(write) => Ok(Self(Driver::new(write))),
            Err(error) => Err(error),
        }
    }

    /// Advance the write by at most one bus action.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Step<()>
    where
        Bus: AnalogRegisterBus<Address = Address>,
    {
        self.0.poll(bus)
    }
}

/// Polled vendor configuration leaf over a bus.
#[must_use = "a configuration does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct Configuration<Commands, Address>(Driver<ConfigurationTransition<Commands, Address>>);

impl<Commands, Address: Copy + Eq> Configuration<Commands, Address>
where
    Commands: ConfigurationCommands<Address = Address>,
{
    pub fn new(commands: Commands) -> Self {
        Self(Driver::new(ConfigurationTransition::new(commands)))
    }

    /// Advance the configuration by at most one bus action.
    ///
    /// # Errors
    ///
    /// A `Modify` command's value does not fit its field; no bus action was
    /// performed for it.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Result<Step<()>, InvalidCommand>
    where
        Bus: AnalogRegisterBus<Address = Address>,
    {
        match self.0.poll(bus) {
            Step::Pending => Ok(Step::Pending),
            Step::Ready(result) => result.map(Step::Ready),
        }
    }
}

/// One of the two analog I2C hosts of a parallel write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelHost {
    First,
    Second,
}

/// Parallel writes on both analog I2C hosts, as `phy_i2c_paral_write`
/// issues them: one command per host, then each host polled idle.
pub trait ParallelAnalogBus {
    /// One pair of commands, the first for each host.
    type Pair: Copy + core::fmt::Debug;

    /// Install the host map of the parallel sequence.
    fn select_parallel_host_map(&mut self);

    /// Restore the normal host map.
    fn restore_host_map(&mut self);

    /// Publish both commands of `pair`.
    fn start_pair(&mut self, pair: Self::Pair);

    /// Whether `host` is executing its command.
    fn is_busy(&self, host: ParallelHost) -> bool;
}

/// The next step of a parallel write sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelAction<Pair> {
    /// Install the parallel host map.
    SelectParallelMap,
    /// Publish both commands of `pair`.
    StartPair(Pair),
    /// Wait until `host` is idle.
    AwaitIdle(ParallelHost),
    /// Restore the normal host map.
    RestoreMap,
    /// The sequence completed.
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParallelPhase {
    Select,
    Start(usize),
    Await(usize, ParallelHost),
    Done,
}

/// `phy_i2c_paral_write_num` between a parallel host map and its
/// restoration: for each pair, publish both commands, then wait for the first
/// host idle and the second host idle.
#[derive(Clone, Copy, Debug)]
pub struct ParallelTransition<Pairs> {
    pairs: Pairs,
    phase: ParallelPhase,
}

impl<Pairs> ParallelTransition<Pairs> {
    /// The sequence of the pairs `pairs(0)`, `pairs(1)`, ... up to the first
    /// `None`.
    pub const fn new(pairs: Pairs) -> Self {
        Self {
            pairs,
            phase: ParallelPhase::Select,
        }
    }

    /// The next step.
    pub fn action<Pair>(&self) -> ParallelAction<Pair>
    where
        Pairs: Fn(usize) -> Option<Pair>,
    {
        match self.phase {
            ParallelPhase::Select => ParallelAction::SelectParallelMap,
            ParallelPhase::Start(index) => match (self.pairs)(index) {
                Some(pair) => ParallelAction::StartPair(pair),
                None => ParallelAction::RestoreMap,
            },
            ParallelPhase::Await(_, host) => ParallelAction::AwaitIdle(host),
            ParallelPhase::Done => ParallelAction::Complete,
        }
    }

    /// The index of the pair being published or awaited.
    pub const fn pair_index(&self) -> Option<usize> {
        match self.phase {
            ParallelPhase::Start(index) | ParallelPhase::Await(index, _) => Some(index),
            _ => None,
        }
    }

    /// Record that the current step completed.
    pub fn advance<Pair>(&mut self)
    where
        Pairs: Fn(usize) -> Option<Pair>,
    {
        self.phase = match self.phase {
            ParallelPhase::Select => ParallelPhase::Start(0),
            ParallelPhase::Start(index) => match (self.pairs)(index) {
                Some(_) => ParallelPhase::Await(index, ParallelHost::First),
                None => ParallelPhase::Done,
            },
            ParallelPhase::Await(index, ParallelHost::First) => {
                ParallelPhase::Await(index, ParallelHost::Second)
            }
            ParallelPhase::Await(index, ParallelHost::Second) => ParallelPhase::Start(index + 1),
            ParallelPhase::Done => ParallelPhase::Done,
        };
    }
}

/// Polled parallel write sequence over a [`ParallelAnalogBus`].
#[must_use = "a parallel write sequence does nothing until it is polled to completion"]
#[derive(Clone, Copy, Debug)]
pub struct ParallelWrites<Pairs>(ParallelTransition<Pairs>);

impl<Pairs> ParallelWrites<Pairs> {
    /// The sequence of the pairs `pairs(0)`, `pairs(1)`, ... up to the first
    /// `None`.
    pub const fn new(pairs: Pairs) -> Self {
        Self(ParallelTransition::new(pairs))
    }

    /// Advance the sequence by at most one bus action.
    pub fn poll<Bus>(&mut self, bus: &mut Bus) -> Step<()>
    where
        Bus: ParallelAnalogBus,
        Pairs: Fn(usize) -> Option<Bus::Pair>,
    {
        match self.0.action() {
            ParallelAction::SelectParallelMap => bus.select_parallel_host_map(),
            ParallelAction::StartPair(pair) => bus.start_pair(pair),
            ParallelAction::AwaitIdle(host) => {
                if bus.is_busy(host) {
                    return Step::Pending;
                }
            }
            ParallelAction::RestoreMap => {
                bus.restore_host_map();
                self.0.advance();
                return Step::Ready(());
            }
            ParallelAction::Complete => return Step::Ready(()),
        }
        self.0.advance();
        Step::Pending
    }
}

#[cfg(test)]
mod tests;
