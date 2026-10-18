//! What a boot observes about itself and about the boot before it.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// A task whose progress the hang watchdog checks while work waits for it.
///
/// Append new slots only; the post-mortem record stores a slot's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum TaskSlot {
    /// The protocol console's command consumer, while a command waits.
    Console,
    /// A session's evidence publication after its traffic ends.
    SessionEvidence,
}

impl TaskSlot {
    pub const ALL: [Self; 2] = [Self::Console, Self::SessionEvidence];

    /// The slot's identifier in post-mortems and failure messages.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::SessionEvidence => "session-evidence",
        }
    }
}

/// A task that made no progress on pending work while the executors ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct TaskStall {
    pub slot: TaskSlot,
    /// How long the work had waited when the watchdog recorded the hang,
    /// its deadline plus the sampling time.
    pub pending_ms: u32,
}

/// Platform reset classification, independent of radio protocol resets.
/// [`BootEvidence::raw_reset_reason`] keeps the chip's own code, so a reason
/// this classification merges or does not name is never lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum ResetReason {
    Other,
    Software,
    MainWatchdog1,
    /// Power-on, which on some chips also covers a brownout or super
    /// watchdog; a post-mortem that survived tells them apart.
    PowerOn,
    Brownout,
    MainWatchdog0,
    RtcWatchdog,
    SuperWatchdog,
    /// The USB Serial/JTAG controller's JTAG or UART side reset the core.
    UsbSerialJtag,
    /// A JTAG debugger reset the CPU.
    Jtag,
    /// An exception inside the exception handler.
    CpuLockup,
}

/// Observed cause of the boot identified by the enclosing envelope, and what
/// the previous boot left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct BootEvidence {
    pub reset_reason: ResetReason,
    /// The chip's reset reason code; 0 when the target did not report one.
    #[serde(default)]
    pub raw_reset_reason: u8,
    /// The record the previous boot kept in reset-retained memory; `None`
    /// when no valid record survived, as after a power-on.
    #[serde(default)]
    pub post_mortem: Option<PostMortemSummary>,
    /// The record the platform's panic entry left for the boot after a
    /// panic, read once by this boot; `None` when the previous boot did not
    /// panic through it.
    #[serde(default)]
    pub platform_panic: Option<PlatformPanic>,
}

/// The platform panic entry's record of the previous boot's panic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct PlatformPanic {
    pub hart: u8,
    /// The panic ran on the hart's interrupt stack.
    pub in_interrupt: bool,
    /// The interrupted instruction, when [`Self::in_interrupt`].
    pub interrupted_pc: Option<u32>,
    /// The end of the location's file path, as recorded.
    pub file: heapless::String<48>,
    pub line: u32,
    pub column: u32,
}

/// The previous boot's post-mortem record, without its checkpoints, which
/// [`super::GetPostMortemCheckpoints`] pages through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct PostMortemSummary {
    /// Boots since the record was created, including the one that wrote it.
    pub boot_count: u32,
    /// Checkpoints held, oldest first, at most [`POST_MORTEM_CHECKPOINTS`].
    pub checkpoints: u8,
    pub fault: Option<Fault>,
}

/// Checkpoints a post-mortem keeps.
pub const POST_MORTEM_CHECKPOINTS: usize = 32;
/// Checkpoints one [`PostMortemCheckpoints`] page carries.
pub const POST_MORTEM_CHECKPOINT_PAGE: usize = 12;
/// Bytes of a checkpoint name.
pub const CHECKPOINT_NAME_BYTES: usize = 16;

/// How the previous boot ended, when it did not end by an ordinary reset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum Fault {
    Hang(HangFault),
    Panic(PanicFault),
}

/// A watchdog found an executor making no progress and reset the chip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct HangFault {
    pub detected_uptime_ms: u32,
    /// One bit per executor that stopped advancing its heartbeat.
    pub stalled_executors: u8,
    /// The context each hart was interrupted in, by hart index.
    pub harts: [HartState; 2],
    /// The stalled hart's preempted instruction at successive checks.
    pub samples: [u32; 16],
    /// The task that stalled while both executors ran, when that was the
    /// hang; `stalled_executors` is then zero.
    pub stalled_task: Option<TaskStall>,
}

/// Where a hart was interrupted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct HartState {
    /// Whether the hart took the sampling interrupt; a hart that did not
    /// has its interrupts masked, and its other fields are zero.
    pub responded: bool,
    pub mepc: u32,
    pub ra: u32,
    pub sp: u32,
    pub mcause: u32,
    pub mstatus: u32,
}

/// The previous boot panicked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct PanicFault {
    pub file: heapless::String<48>,
    pub line: u32,
    /// The panic's message when it is a static string; empty for a message
    /// with arguments, which the panic path does not format.
    pub message: heapless::String<96>,
}

/// A named point the previous boot passed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct Checkpoint {
    pub name: heapless::String<CHECKPOINT_NAME_BYTES>,
    pub arg: u32,
    pub uptime_ms: u32,
    pub hart: u8,
}

/// Checkpoints `first..` of the previous boot's post-mortem, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct PostMortemCheckpoints {
    pub first: u8,
    pub checkpoints: heapless::Vec<Checkpoint, POST_MORTEM_CHECKPOINT_PAGE>,
}
