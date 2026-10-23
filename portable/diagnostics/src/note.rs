//! Typed HIL diagnostic notes: a topic and its values, with no target
//! formatting. The producer builds a [`Note`] with its topic's constructor
//! (argument count and kinds are checked by the compiler); the host looks the
//! topic up in the `render` feature's layout and prints the text.

/// Most 32-bit words one note carries.
pub const MAX_WORDS: usize = 8;
/// Most names (static text such as a register or a variant) in one note.
pub const MAX_NAMES: usize = 6;
/// Longest name in one note.
pub const MAX_NAME: usize = 64;
/// Most name bytes in one note, all names together.
pub const MAX_NAME_BYTES: usize = 96;

/// A topic's value, as the host prints it.
#[cfg(feature = "render")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    /// Decimal.
    Dec,
    /// Lower-case hexadecimal, zero-padded to the width.
    Hex(u8),
    /// `0x`-prefixed hexadecimal; the width counts the prefix (0: unpadded).
    PrefixedHex(u8),
    /// `true` or `false`.
    Flag,
    /// `1` or `0`.
    Bit,
    /// A MAC address's last two bytes as `xx:xx:xx:xx:EE:FF`, the rest masked.
    MacTail,
    /// An IPv4 address in the word, most significant byte first.
    Ipv4,
    /// A 64-bit decimal in two words, low word first.
    Wide,
    /// A signed decimal.
    Signed,
    /// `None`, or `Some(n)` for the word `n + 1`.
    Optional,
    /// The note's next name.
    Name,
    /// The rest of the note: a nested error's variant path and its numbers.
    Path,
}

/// One note: its topic, and the 32-bit words and names its topic's layout
/// asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Note<'a> {
    pub topic: Topic,
    words: [u32; MAX_WORDS],
    count: u8,
    names: [&'a str; MAX_NAMES],
    name_count: u8,
    name_bytes: u8,
    overflow: bool,
}

impl<'a> Note<'a> {
    pub(crate) fn new(topic: Topic) -> Self {
        Self {
            topic,
            words: [0; MAX_WORDS],
            count: 0,
            names: [""; MAX_NAMES],
            name_count: 0,
            name_bytes: 0,
            overflow: false,
        }
    }

    /// The note a decoder read: the caller checked the counts.
    pub(crate) fn from_parts(
        topic: Topic,
        words: &[u32],
        names: &[&'a str],
        overflow: bool,
    ) -> Self {
        let mut note = Self::new(topic);
        note.overflow = overflow;
        for &word in words {
            note = note.word(word);
        }
        for &name in names {
            note = note.named(name);
        }
        note
    }

    /// The words, in the topic's field order.
    pub fn words(&self) -> &[u32] {
        self.words.get(..usize::from(self.count)).unwrap_or(&[])
    }

    /// The names, in the topic's field order.
    pub fn names(&self) -> &[&'a str] {
        self.names
            .get(..usize::from(self.name_count))
            .unwrap_or(&[])
    }

    fn word(mut self, value: u32) -> Self {
        let slot = self.words.get_mut(usize::from(self.count));
        assert!(slot.is_some(), "a note's words fit MAX_WORDS");
        if let Some(slot) = slot {
            *slot = value;
            self.count += 1;
        }
        self
    }

    fn wide(self, value: u64) -> Self {
        self.word(value as u32).word((value >> 32) as u32)
    }

    fn named(mut self, name: &'a str) -> Self {
        let slot = self.names.get_mut(usize::from(self.name_count));
        assert!(
            name.len() <= MAX_NAME
                && usize::from(self.name_bytes) + name.len() <= MAX_NAME_BYTES
                && slot.is_some(),
            "a note's names fit MAX_NAME, MAX_NAME_BYTES and MAX_NAMES"
        );
        if let Some(slot) = slot {
            *slot = name;
            self.name_count += 1;
            self.name_bytes += name.len() as u8;
        }
        self
    }

    /// Appends the path's names and numbers. What does not fit the note's
    /// name budget makes the note say it overflowed; nothing else is lost.
    fn pathed(mut self, path: &crate::DiagPath) -> Self {
        self.overflow |= path.overflowed();
        for &name in path.names() {
            if name.len() > MAX_NAME
                || self.name_bytes as usize + name.len() > MAX_NAME_BYTES
                || self.name_count as usize >= MAX_NAMES
            {
                self.overflow = true;
                break;
            }
            self = self.named(name);
        }
        for &word in path.words() {
            if self.count as usize >= MAX_WORDS {
                self.overflow = true;
                break;
            }
            self = self.word(word);
        }
        self
    }

    /// Whether the topic's path lost something that did not fit.
    pub const fn overflowed(&self) -> bool {
        self.overflow
    }
}

macro_rules! argument {
    (path) => { &crate::DiagPath };
    (dec) => { u32 };
    (signed) => { i32 };
    (opt) => { Option<u8> };
    (hex8) => { u32 };
    (hex2) => { u32 };
    (hexa) => { u32 };
    (hexa4) => { u32 };
    (hexa10) => { u32 };
    (flag) => { bool };
    (bit) => { bool };
    (mactail) => { [u8; 2] };
    (ipv4) => { [u8; 4] };
    (wide) => { u64 };
    (name) => { &'a str };
}

macro_rules! push {
    ($note:ident, $value:ident, path) => {
        $note.pathed($value)
    };
    ($note:ident, $value:ident, dec) => {
        $note.word($value)
    };
    ($note:ident, $value:ident, signed) => {
        $note.word($value as u32)
    };
    ($note:ident, $value:ident, opt) => {
        $note.word(match $value {
            Some(value) => value as u32 + 1,
            None => 0,
        })
    };
    ($note:ident, $value:ident, hex8) => {
        $note.word($value)
    };
    ($note:ident, $value:ident, hex2) => {
        $note.word($value)
    };
    ($note:ident, $value:ident, hexa) => {
        $note.word($value)
    };
    ($note:ident, $value:ident, hexa4) => {
        $note.word($value)
    };
    ($note:ident, $value:ident, hexa10) => {
        $note.word($value)
    };
    ($note:ident, $value:ident, flag) => {
        $note.word($value as u32)
    };
    ($note:ident, $value:ident, bit) => {
        $note.word($value as u32)
    };
    ($note:ident, $value:ident, mactail) => {
        $note.word(u32::from(u16::from_be_bytes($value)))
    };
    ($note:ident, $value:ident, ipv4) => {
        $note.word(u32::from_be_bytes($value))
    };
    ($note:ident, $value:ident, wide) => {
        $note.wide($value)
    };
    ($note:ident, $value:ident, name) => {
        $note.named($value)
    };
}

macro_rules! words_of {
    (wide) => {
        2
    };
    (name) => {
        0
    };
    (path) => {
        0
    };
    ($other:ident) => {
        1
    };
}

macro_rules! names_of {
    (name) => {
        1
    };
    (path) => {
        0
    };
    ($other:ident) => {
        0
    };
}

macro_rules! has_path {
    (path) => {
        1
    };
    ($other:ident) => {
        0
    };
}

#[cfg(feature = "render")]
macro_rules! field_of {
    (path) => {
        Field::Path
    };
    (dec) => {
        Field::Dec
    };
    (signed) => {
        Field::Signed
    };
    (opt) => {
        Field::Optional
    };
    (hex8) => {
        Field::Hex(8)
    };
    (hex2) => {
        Field::Hex(2)
    };
    (hexa) => {
        Field::PrefixedHex(0)
    };
    (hexa4) => {
        Field::PrefixedHex(4)
    };
    (hexa10) => {
        Field::PrefixedHex(10)
    };
    (flag) => {
        Field::Flag
    };
    (bit) => {
        Field::Bit
    };
    (mactail) => {
        Field::MacTail
    };
    (ipv4) => {
        Field::Ipv4
    };
    (wide) => {
        Field::Wide
    };
    (name) => {
        Field::Name
    };
}

macro_rules! topics {
    ($(
        $(#[$meta:meta])*
        $variant:ident = $code:literal, $constructor:ident, $prefix:literal
            ($($key:literal $field:ident: $kind:ident),* $(,)?);
    )*) => {
        /// The kind of a note: the line shape the host prints for it.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u16)]
        pub enum Topic {
            $($(#[$meta])* $variant = $code,)*
        }

        impl Topic {
            pub const fn from_raw(raw: u16) -> Option<Self> {
                match raw {
                    $($code => Some(Self::$variant),)*
                    _ => None,
                }
            }

            pub const fn raw(self) -> u16 {
                self as u16
            }

            /// How many 32-bit words a note of this topic carries.
            pub const fn word_count(self) -> usize {
                match self {
                    $(Self::$variant => 0 $(+ words_of!($kind))*,)*
                }
            }

            /// Whether the topic ends with a variant path, which adds names and
            /// words to the fixed ones.
            pub const fn has_path(self) -> bool {
                match self {
                    $(Self::$variant => 0 $(+ has_path!($kind))* != 0,)*
                }
            }

            /// How many names a note of this topic carries.
            pub const fn name_count(self) -> usize {
                match self {
                    $(Self::$variant => 0 $(+ names_of!($kind))*,)*
                }
            }

            /// The text before the first field, and the fields: key and kind.
            #[cfg(feature = "render")]
            pub const fn layout(self) -> (&'static str, &'static [(&'static str, Field)]) {
                match self {
                    $(Self::$variant => ($prefix, &[$(($key, field_of!($kind))),*]),)*
                }
            }
        }

        impl<'a> Note<'a> {
            $(
                $(#[$meta])*
                #[allow(clippy::too_many_arguments)]
                pub fn $constructor($($field: argument!($kind)),*) -> Note<'a> {
                    let note = Note::new(Topic::$variant);
                    $(let note = push!(note, $field, $kind);)*
                    note
                }
            )*
        }

        // The fixed part of every topic fits a note; a path fills what is
        // left, and the decoder refuses a count past the arrays.
        $(const _: () = {
            let words: usize = 0 $(+ words_of!($kind))*;
            let names: usize = 0 $(+ names_of!($kind))*;
            assert!(words <= MAX_WORDS, "a topic's words fit MAX_WORDS");
            assert!(names <= MAX_NAMES, "a topic's names fit MAX_NAMES");
        };)*

        #[cfg(test)]
        pub(crate) const ALL_TOPICS: &[Topic] = &[$(Topic::$variant),*];
    };
}

topics! {
    /// The stack watchpoint the runtime armed.
    StackGuard = 1, stack_guard, "OPEN_RADIO_HIL stack_guard"
        ("hart" hart: dec, "control" control: hex8, "address" address: hex8);
    /// The previous boot's fatal exception.
    FatalException = 2, fatal_exception, "OPEN_RADIO_HIL runtime=EXCEPTION boot=previous"
        ("hart" hart: dec, "mcause" mcause: hex8, "mepc" mepc: hex8, "mtval" mtval: hex8,
         "ra" ra: hex8, "mscratch" mscratch: hex8);
    /// The previous boot's panic.
    FatalPanic = 3, fatal_panic, "OPEN_RADIO_HIL runtime=PANIC boot=previous"
        ("hart" hart: dec, "stage" stage: dec, "mcause" mcause: hex8, "mepc" mepc: hex8,
         "mtval" mtval: hex8);
    FatalIrqVector = 4, fatal_irq_vector, "fatal irq_vector"
        ("vector" vector: dec, "line" line: hex8, "mtvec" mtvec: hex8, "mtvt" mtvt: hex8,
         "slot" slot: hex8);
    FatalIrqPending = 5, fatal_irq_pending, "fatal irq_pending words="
        ("" w0: hex8, "" w1: hex8, "" w2: hex8, "" w3: hex8, "" w4: hex8, "" w5: hex8);
    FatalIrqRouted = 6, fatal_irq_routed, "fatal irq_routed words="
        ("" w0: hex8, "" w1: hex8, "" w2: hex8, "" w3: hex8, "" w4: hex8, "" w5: hex8);
    /// Bytes the synchronous USB writer had to drop because the FIFO was full.
    DiagLost = 9, diag_lost, "OPEN_RADIO_HIL diag-lost"
        ("bytes" bytes: dec);
    /// A HIL runtime started on a placement profile.
    RuntimeStart = 10, runtime_start, "OPEN_RADIO_HIL runtime=START"
        ("profile" profile: name);
    /// A diagnostic image's runtime started.
    DiagnosticStart = 11, diagnostic_start, "OPEN_RADIO_HIL runtime=START"
        ("diagnostic" diagnostic: name);
    /// A HIL runtime stopped at a failed boot check.
    RuntimeFail = 12, runtime_fail, "OPEN_RADIO_HIL runtime=FAIL"
        ("reason" reason: name);
    /// The runtime's placement probes passed.
    PlacementPass = 13, placement_pass,
        "OPEN_RADIO_HIL placement=PASS isr=SRAM dma_probes=SRAM task_stack=PSRAM irq_stack=SRAM" ();
    /// The entropy source passed its health test.
    EntropyPass = 14, entropy_pass, "OPEN_RADIO_HIL entropy=PASS" ();
    /// A physical-layer step failed without an error to report.
    PhyFail = 15, phy_fail, "OPEN_RADIO_HIL phy=FAIL"
        ("step" step: name);
    /// W3 console block: the service's receive diagnostics.
    W3Rx = 100, w3_rx, "w3.rx"
        ("queue_full" queue_full: dec, "rejected" rejected: dec, "reasons" reasons: dec,
         "port_events_lost" port_events_lost: dec);
    W3Diag = 101, w3_diag, "w3.diag"
        ("malformed_ip" malformed_ip: dec, "queue_drops" queue_drops: dec);
    W3Begin = 102, w3_begin, "OER-W3-BEGIN v=1" ();
    W3Config = 103, w3_config, "w3.config"
        ("channel" channel: dec, "target" target: mactail, "stand" stand: mactail);
    W3Association = 104, w3_association, "w3.association"
        ("authenticated" authenticated: bit, "associated" associated: bit);
    W3Lease = 105, w3_lease, "w3.lease"
        ("acquired" acquired: bit, "address" address: ipv4, "router" router: ipv4,
         "prefix" prefix: dec);
    W3Ping = 106, w3_ping, "w3.ping"
        ("sent" sent: dec, "received" received: dec, "interval_ms" interval_ms: dec);
    W3PeerPing = 107, w3_peer_ping, "w3.peer_ping"
        ("requests" requests: dec, "replies" replies: dec);
    W3Stats = 108, w3_stats, "w3.stats"
        ("tx_timeout" tx_timeout: dec, "queue_collision" queue_collision: dec,
         "irq_free" irq_free: dec);
    W3Departure = 109, w3_departure, "w3.departure"
        ("deauth_sent" deauth_sent: bit);
    W3Gate = 110, w3_gate, "w3.gate"
        ("id" id: name, "pass" pass: bit);
    W3End = 111, w3_end, "OER-W3-END lines=12" ();
    W3Pass = 112, w3_pass, "OER-RESULT: pass w3" ();
    W3Fail = 113, w3_fail, "OER-RESULT: fail w3" ();
    /// The smoke image's Embassy timer rearming.
    Rearm = 7, rearm, "OPEN_RADIO_HIL rearm"
        ("step" step: dec, "delay_ms" delay_ms: wide, "elapsed_us" elapsed_us: wide);
    RearmFailed = 8, rearm_failed, "OPEN_RADIO_HIL boot-smoke=FAIL rearm=FAIL"
        ("step" step: dec, "elapsed_us" elapsed_us: wide);

    CcaCheckpoint = 20, cca_checkpoint, "OPEN_RADIO_HIL phy=cca-checkpoint"
        ("point" point: name, "sample" sample: opt, "micros" micros: wide, "mac_rx" mac_rx: flag,
         "reload" reload: flag, "base" base: hexa, "next" next: hexa, "last" last: hexa);
    CcaRegister = 21, cca_register, "OPEN_RADIO_HIL phy=cca-register"
        ("point" point: name, "sample" sample: opt, "name" register: name, "raw" raw: hexa10);
    CcaEnable = 22, cca_enable, "OPEN_RADIO_HIL phy=cca-enable"
        ("point" point: name, "sample" sample: opt, "force" force: dec, "aux_force" aux_force: dec,
         "mac_active" mac_active: dec);
    CcaEnableRegister = 23, cca_enable_register, "OPEN_RADIO_HIL phy=cca-enable-register"
        ("point" point: name, "sample" sample: opt, "name" register: name, "raw" raw: hexa10);
    Sar2Checkpoint = 24, sar2_checkpoint, "OPEN_RADIO_HIL phy=sar2-checkpoint"
        ("point" point: name, "raw" raw: hexa4, "init" init: dec);
    TxRateCheckpoint = 25, tx_rate_checkpoint, "OPEN_RADIO_HIL phy=tx-rate-checkpoint"
        ("point" point: name, "raw" raw: hexa10, "low" low: hexa, "high" high: hexa);
    CcaRxFail = 26, cca_rx_fail, "OPEN_RADIO_HIL phy=cca-rx FAIL"
        ("step" step: name, "error" error: path);
    CcaRxBegin = 27, cca_rx_begin, "OPEN_RADIO_HIL phy=cca-rx begin"
        ("channel" channel: dec, "descriptors" descriptors: dec, "capacity" capacity: dec);
    CcaRxCleanup = 28, cca_rx_cleanup, "OPEN_RADIO_HIL phy=cca-rx cleanup mac_idle=true"
        ("gate_closed" gate_closed: flag, "buffers_returned" buffers_returned: flag,
         "buffers" buffers: dec);
    CcaRxSample = 29, cca_rx_sample, "OPEN_RADIO_HIL phy=cca-rx"
        ("sample" sample: dec, "received" received: dec, "normalized" normalized: dec,
         "beacons" beacons: dec);
    Registration = 30, registration, "OPEN_RADIO_HIL phy=registration"
        ("full" full: flag, "path" path: path);
    EnterWifi = 31, enter_wifi, "OPEN_RADIO_HIL phy=enter radio=Wifi"
        ("channel" channel: dec);
    EnterIeee802154 = 32, enter_ieee802154, "OPEN_RADIO_HIL phy=enter radio=Ieee802154"
        ("channel" channel: dec, "power_dbm" power_dbm: signed);
    HoldFail = 33, hold_fail, "OPEN_RADIO_HIL hold=FAIL"
        ("tick" tick: dec, "result" result: path);
    CaptureStop = 34, capture_stop, "OPEN_RADIO_HIL capture-stop"
        ("observation" observation: name, "close" close: name, "rf-closed" rf_closed: flag);
    Ieee802154JoinFail = 35, ieee802154_join_fail, "OPEN_RADIO_HIL phy=FAIL step=154-join"
        ("error" error: path);
    Ieee802154MaintainFail = 36, ieee802154_maintain_fail,
        "OPEN_RADIO_HIL phy=FAIL step=154-maintain" ("error" error: path);
    Joined = 37, joined, "OPEN_RADIO_HIL phy=joined" ("acquired" acquired: path);

    T7PreparePolls = 40, t7_prepare_polls, "OPEN_RADIO_HIL t7-prepare"
        ("polls" polls: dec, "max-poll-us" max_poll_us: dec);
    T7PrepareResumes = 41, t7_prepare_resumes, "OPEN_RADIO_HIL t7-prepare"
        ("resume-count" resumes: dec, "late-max-us" late_max_us: dec,
         "late-total-us" late_total_us: dec);
    T7PrepareReady = 42, t7_prepare_ready, "OPEN_RADIO_HIL t7-prepare"
        ("ready-count" ready: dec, "ready-max-us" ready_max_us: dec);
    T7PrepareCommands = 43, t7_prepare_commands, "OPEN_RADIO_HIL t7-prepare"
        ("commands" commands: dec, "first-check-max-us" first_check_max_us: dec);
    T7PrepareYields = 44, t7_prepare_yields, "OPEN_RADIO_HIL t7-prepare"
        ("yield-count" yields: dec, "yield-total-us" total_us: dec, "yield-max-us" max_us: dec);
    T7PrepareFailureTimes = 45, t7_prepare_failure_times, "OPEN_RADIO_HIL t7-prepare"
        ("failure-elapsed-us" elapsed_us: dec, "failure-late-us" late_us: dec);
    T7PrepareFailureBusy = 46, t7_prepare_failure_busy, "OPEN_RADIO_HIL t7-prepare"
        ("failure-busy" busy: opt, "complete" complete: flag);
    T7PrepareNotReady = 47, t7_prepare_not_ready, "OPEN_RADIO_HIL t7-prepare not-ready"
        ("command" command: dec, "elapsed-us" elapsed_us: dec);
    T7TimerRegistrations = 48, t7_timer_registrations, "OPEN_RADIO_HIL t7-timer"
        ("registrations" registrations: dec, "due-at-register" due: dec, "irqs" irqs: dec);
    T7TimerProgram = 49, t7_timer_program, "OPEN_RADIO_HIL t7-timer"
        ("program-max-us" program_us: dec, "dispatch-max-us" dispatch_us: dec,
         "invalid" invalid: dec);
    T7TimerLatency = 50, t7_timer_latency, "OPEN_RADIO_HIL t7-timer"
        ("alarm-to-irq-max-us" alarm_to_irq_us: dec, "irq-late-max-us" irq_late_us: dec,
         "irq-to-dispatch-max-us" irq_to_dispatch_us: dec);
    T7TimerUnmatched = 51, t7_timer_unmatched, "OPEN_RADIO_HIL t7-timer"
        ("unmatched-irqs" irqs: dec, "unmatched-dispatch" dispatches: dec, "armed" armed: dec);

    GainParts = 60, gain_parts, "OPEN_RADIO_HIL gain-parts"
        ("family" family: dec, "lower-us" lower_us: dec, "arithmetic-us" arithmetic_us: dec,
         "publication-us" publication_us: dec);
    SarTailBins = 61, sar_tail_bins, "OPEN_RADIO_HIL sar_tail"
        ("tick" tick: dec, "first" first: dec, "bins" b0: dec, "" b1: dec, "" b2: dec, "" b3: dec,
         "" b4: dec, "" b5: dec);
    SarTailLargest = 62, sar_tail_largest, "OPEN_RADIO_HIL sar_tail"
        ("tick" tick: dec, "first" first: dec, "largest" l0: wide, "" l1: wide);
    SarTailTotals = 63, sar_tail_totals, "OPEN_RADIO_HIL sar_tail"
        ("tick" tick: dec, "total" total: dec, "expired" expired: dec,
         "max_observations" max_observations: dec);
    EnginePolls = 64, engine_polls, "OPEN_RADIO_HIL engine"
        ("family" family: dec, "polls" polls: dec, "pending" pending: dec, "ready" ready: dec);
    EngineTotals = 65, engine_totals, "OPEN_RADIO_HIL engine"
        ("family" family: dec, "total-us" total_us: dec, "max-us" max_us: dec);
    EngineResumes = 66, engine_resumes, "OPEN_RADIO_HIL engine"
        ("family" family: dec, "resumes" resumes: dec, "late-total-us" late_total_us: dec);
    EngineLateness = 67, engine_lateness, "OPEN_RADIO_HIL engine"
        ("family" family: dec, "late-max-us" late_max_us: dec, "invalid" invalid: flag);
    PowerGain = 68, power_gain, "OPEN_RADIO_HIL power-gain"
        ("family" family: dec, "count" count: dec, "max-us" max_us: dec);
    DcodeAtom = 69, dcode_atom, "OPEN_RADIO_HIL dcode-atom"
        ("count" count: dec, "max-us" max_us: dec);
    CalibrationAtom = 70, calibration_atom, "OPEN_RADIO_HIL calibration-atom"
        ("family" family: dec, "count" count: dec, "max-us" max_us: dec);
    ChannelAction = 71, channel_action, "OPEN_RADIO_HIL channel-action"
        ("name" action: name, "packed" packed: wide);
    RxGainParts = 72, rx_gain_parts, "OPEN_RADIO_HIL rx-gain-parts"
        ("lower-us" lower_us: dec, "hardware-us" hardware_us: dec, "commit-us" commit_us: dec);
    RxI2cSamples = 73, rx_i2c_samples, "OPEN_RADIO_HIL"
        ("rx-i2c-samples-packed" samples: wide);
    CalibrationName = 74, calibration_name, "OPEN_RADIO_HIL calibration-name channel=false"
        ("part" part: dec, "action" action: name, "first" first: flag, "us" micros: dec);
    HoldPoll = 75, hold_poll, "OPEN_RADIO_HIL hold-poll"
        ("tick" tick: dec, "op" operation: dec, "polls" polls: dec, "pending" pending: dec,
         "resumes" resumes: dec);
    HoldGap = 76, hold_gap, "OPEN_RADIO_HIL hold-gap"
        ("tick" tick: dec, "op" operation: dec, "total-us" total_us: wide, "max-us" max_us: wide,
         "invalid" invalid: flag);
    HoldRoot = 77, hold_root, "OPEN_RADIO_HIL hold-root"
        ("tick" tick: dec, "poll-us" poll_us: wide, "max-poll-us" max_poll_us: wide);
}

#[cfg(test)]
mod tests;
