//! Typed HIL diagnostic notes: a topic and its values, with no target
//! formatting. The producer builds a [`Note`] with its topic's constructor
//! (argument count and kinds are checked by the compiler); the host looks the
//! topic up in the `render` feature's layout and prints the text.

/// How a topic's text joins the console line it belongs to: a long line is
/// several notes, and the host prints their texts as one line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Part {
    /// The note is a whole line.
    Whole,
    /// The note starts a line that the next note continues.
    First,
    /// The note continues the line and the next note continues it further.
    Middle,
    /// The note ends the line it continues.
    Last,
}

/// The reasons a unit was dropped or refused, in the order of the discard
/// counts a note carries: the runner's drops first, then the application's
/// RX-control prefix refusals.
pub const DISCARDS: [&str; 15] = [
    "empty",
    "too_long",
    "chained",
    "oversized",
    "overload",
    "rx_failure",
    "mic_failure",
    "quarantined",
    "invalid",
    "prefix_chain",
    "bounds",
    "output_small",
    "metadata",
    "ignored",
    "unsupported",
];

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
    /// Two ASCII characters in the word, the first in the high byte.
    Ascii2,
    /// Four signed bytes in the word, the first in the high byte, as `a,b,c,d`.
    I8x4,
    /// A byte list of at most 8 in a length word and two words, as lower-case
    /// hexadecimal pairs, or `-` when empty.
    Bytes8,
    /// As [`Field::Bytes8`], for at most 16 bytes in a length word and four.
    Bytes16,
    /// A present flag and a number: the decimal, or `-` without one.
    Dash,
    /// A present flag and a number: `0x` and the hexadecimal, or `-`.
    HexDash,
    /// A present flag and a signed number, or `-`.
    SignedDash,
    /// `-` for 0, `0` for 1 and `1` for 2.
    BitDash,
    /// A present flag, a minimum and a maximum: `min..max`, or `-`.
    Range,
    /// As [`Field::Range`], for signed numbers.
    SignedRange,
    /// A 64-bit lower and upper bound: the number when they agree,
    /// otherwise `lower..upper`.
    Over,
    /// Two numbers as `AxB`.
    By,
    /// A count and up to sixteen descriptor indices, a byte each (255 for an
    /// unknown one), as `1,x,3`, or `-` without one.
    Indices,
    /// `-` for 0, `x` for 1, and `n` for the word `n + 2`.
    IndexDash,
    /// The first eight of the fifteen discard counts: the nonzero ones as
    /// `reason:count` joined by `,`.
    DiscardsLow,
    /// A separator flag, then the last seven discard counts, as the nonzero
    /// `reason:count`; the flag asks for a `,` before the first one.
    DiscardsHigh,
    /// A name whose `_` are printed as `-`.
    Phrase,
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

    /// A length word and `words` words of the bytes, first byte in the high
    /// byte of the first word; at most `4 * words` bytes are kept.
    fn byte_words(self, bytes: &[u8], words: usize) -> Self {
        let kept = bytes.get(..bytes.len().min(4 * words)).unwrap_or(&[]);
        let mut note = self.word(kept.len() as u32);
        for index in 0..words {
            let mut word = [0; 4];
            for (slot, byte) in word.iter_mut().zip(kept.iter().skip(4 * index)) {
                *slot = *byte;
            }
            note = note.word(u32::from_be_bytes(word));
        }
        note
    }

    /// A count word and the first sixteen indices, a byte each, four to a
    /// word with the first in the high byte; an unknown index is 255. An
    /// index past 254 or an entry past sixteen would not fit and is kept as
    /// unknown: the ring is smaller than that.
    fn index_words(self, indices: &[Option<usize>]) -> Self {
        let kept = indices.get(..indices.len().min(16)).unwrap_or(&[]);
        let mut note = self.word(kept.len() as u32);
        for chunk in 0..4 {
            let mut word = [0xff; 4];
            for (slot, index) in word.iter_mut().zip(kept.iter().skip(4 * chunk)) {
                *slot = match index {
                    Some(index) if *index < 255 => *index as u8,
                    _ => 255,
                };
            }
            note = note.word(u32::from_be_bytes(word));
        }
        note
    }

    /// `count` discard counts of `counts` from `first`.
    fn discard_words(mut self, counts: &[u32; 15], first: usize, count: usize) -> Self {
        for index in first..first + count {
            self = self.word(counts.get(index).copied().unwrap_or(0));
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
    (ascii2) => { [u8; 2] };
    (dash) => { Option<u32> };
    (hexdash) => { Option<u32> };
    (sdash) => { Option<i32> };
    (bitdash) => { Option<bool> };
    (range) => { Option<(u32, u32)> };
    (srange) => { Option<(i32, i32)> };
    (over) => { (u64, u64) };
    (by) => { (u32, u32) };
    (indices) => { &[Option<usize>] };
    (indexdash) => { Option<Option<usize>> };
    (discards_lo) => { &[u32; 15] };
    (discards_hi) => { (&[u32; 15], bool) };
    (phrase) => { &'a str };
    (i8x4) => { [i8; 4] };
    (bytes8) => { &[u8] };
    (bytes16) => { &[u8] };
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
    ($note:ident, $value:ident, dash) => {
        $note
            .word($value.is_some() as u32)
            .word($value.unwrap_or(0))
    };
    ($note:ident, $value:ident, hexdash) => {
        $note
            .word($value.is_some() as u32)
            .word($value.unwrap_or(0))
    };
    ($note:ident, $value:ident, sdash) => {
        $note
            .word($value.is_some() as u32)
            .word($value.unwrap_or(0) as u32)
    };
    ($note:ident, $value:ident, bitdash) => {
        $note.word(match $value {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        })
    };
    ($note:ident, $value:ident, range) => {
        $note
            .word($value.is_some() as u32)
            .word($value.map_or(0, |range| range.0))
            .word($value.map_or(0, |range| range.1))
    };
    ($note:ident, $value:ident, srange) => {
        $note
            .word($value.is_some() as u32)
            .word($value.map_or(0, |range| range.0) as u32)
            .word($value.map_or(0, |range| range.1) as u32)
    };
    ($note:ident, $value:ident, over) => {
        $note.wide($value.0).wide($value.1)
    };
    ($note:ident, $value:ident, by) => {
        $note.word($value.0).word($value.1)
    };
    ($note:ident, $value:ident, indices) => {
        $note.index_words($value)
    };
    ($note:ident, $value:ident, indexdash) => {
        $note.word(match $value {
            None => 0,
            Some(None) => 1,
            Some(Some(index)) => (index as u32).saturating_add(2),
        })
    };
    ($note:ident, $value:ident, discards_lo) => {
        $note.discard_words($value, 0, 8)
    };
    ($note:ident, $value:ident, discards_hi) => {
        $note.word($value.1 as u32).discard_words($value.0, 8, 7)
    };
    ($note:ident, $value:ident, phrase) => {
        $note.named($value)
    };
    ($note:ident, $value:ident, ascii2) => {
        $note.word(u32::from(u16::from_be_bytes($value)))
    };
    ($note:ident, $value:ident, i8x4) => {
        $note.word(u32::from_be_bytes($value.map(|byte| byte as u8)))
    };
    ($note:ident, $value:ident, bytes8) => {
        $note.byte_words($value, 2)
    };
    ($note:ident, $value:ident, bytes16) => {
        $note.byte_words($value, 4)
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
    (bytes8) => {
        3
    };
    (dash) => {
        2
    };
    (hexdash) => {
        2
    };
    (sdash) => {
        2
    };
    (range) => {
        3
    };
    (srange) => {
        3
    };
    (over) => {
        4
    };
    (by) => {
        2
    };
    (indices) => {
        5
    };
    (discards_lo) => {
        8
    };
    (discards_hi) => {
        8
    };
    (phrase) => {
        0
    };
    (bytes16) => {
        5
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
    (phrase) => {
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
    (ascii2) => {
        Field::Ascii2
    };
    (dash) => {
        Field::Dash
    };
    (hexdash) => {
        Field::HexDash
    };
    (sdash) => {
        Field::SignedDash
    };
    (bitdash) => {
        Field::BitDash
    };
    (range) => {
        Field::Range
    };
    (srange) => {
        Field::SignedRange
    };
    (over) => {
        Field::Over
    };
    (by) => {
        Field::By
    };
    (indices) => {
        Field::Indices
    };
    (indexdash) => {
        Field::IndexDash
    };
    (discards_lo) => {
        Field::DiscardsLow
    };
    (discards_hi) => {
        Field::DiscardsHigh
    };
    (phrase) => {
        Field::Phrase
    };
    (i8x4) => {
        Field::I8x4
    };
    (bytes8) => {
        Field::Bytes8
    };
    (bytes16) => {
        Field::Bytes16
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
    /// The previous boot's failed boot check, kept while the console's
    /// driver held the USB block.
    FatalFail = 17, fatal_fail, "OPEN_RADIO_HIL runtime=FAIL boot=previous"
        ("hart" hart: dec, "reason" reason: name);
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
    /// Device wall time for one cooperative WPA2 PMK derivation.
    StationPmk = 16, station_pmk, "OPEN_RADIO_HIL station-pmk"
        ("elapsed_us" elapsed_us: wide);
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
    W3Probes = 114, w3_probes, "w3.probes"
        ("socket_errors" socket_errors: dec, "timeouts" timeouts: dec, "last_socket_error" last_socket_error: name);
    W3Bridge = 115, w3_bridge, "w3.bridge"
        ("turns" turns: dec, "rx_frames" rx_frames: dec, "rx_max" rx_max: dec);

    /// W2 console block.
    W2Begin = 120, w2_begin, "OER-W2-BEGIN v=1" ();
    W2Config = 121, w2_config, "w2.config"
        ("channel" channel: dec, "country" country: ascii2, "cap" cap: signed,
         "antenna_mbm" antenna_mbm: signed, "chip_mbm" chip_mbm: signed,
         "target" target: mactail, "stand" stand: mactail);
    W2ChannelFirst = 122, w2_channel_first, "w2.channel"
        ("channel" channel: dec, "sequence" sequence: dec, "allowed" allowed: bit,
         "sent" sent: dec, "success" success: bit, "responses" responses: dec);
    W2ChannelLast = 123, w2_channel_last, ""
        ("rate_kbps" rate_kbps: dec, "retune_us" retune_us: wide, "power" power: i8x4,
         "ceiling" ceiling: signed);
    W2Basic = 124, w2_basic, "w2.basic"
        ("supported" supported: bytes8, "extended" extended: bytes16);
    W2Unicast = 125, w2_unicast, "w2.unicast"
        ("rate_kbps" rate_kbps: dec, "sequence" sequence: dec, "success" success: bit,
         "responses" responses: dec, "attempts" attempts: dec, "raw" raw: dec,
         "power" power: i8x4, "ceiling" ceiling: signed);
    W2Stats = 126, w2_stats, "w2.stats"
        ("timeouts" timeouts: dec, "collisions" collisions: dec, "colors" colors: dec,
         "errors" errors: dec, "irq_free" irq_free: dec, "error" error: name);
    W2Gate = 127, w2_gate, "w2.gate"
        ("name" name: name, "pass" pass: bit);
    W2End = 128, w2_end, "OER-W2-END"
        ("lines" lines: dec);
    W2Pass = 129, w2_pass, "OER-RESULT: pass w2" ();
    W2Fail = 130, w2_fail, "OER-RESULT: fail w2" ();
    /// W0 console: the Wi-Fi client's bring-up evidence.
    W0Identity = 140, w0_identity, "OER w0 identity"
        ("rf_cal_version" rf_cal_version: dec, "base_mac" base_mac: bytes8,
         "mac_ext" mac_ext: hexa);
    W0Join = 141, w0_join, "OER w0 join registered"
        ("epoch" epoch: dec, "calibration_path" calibration_path: name,
         "registration_us" registration_us: wide);
    W0JoinPhy = 142, w0_join_phy, "OER w0 join"
        ("phy" phy: name, "micros" micros: wide);
    W0ReadyWaits = 143, w0_ready_waits, "OER w0 ready_waits"
        ("tx_dc_max_not_ready" tx_dc_max_not_ready: dec,
         "pwdet_sar_max_not_ready" pwdet_sar_max_not_ready: dec);
    W0RfCalibration = 144, w0_rf_calibration, "OER w0 rf_calibration"
        ("charge_pump_locked" charge_pump_locked: flag, "frequency" frequency: flag,
         "table_entries" table_entries: dec);
    W0RfCalibrationNone = 145, w0_rf_calibration_none, "OER w0 rf_calibration None" ();
    W0RfPoint = 146, w0_rf_point, "OER w0 rf_calibration_point"
        ("point" point: name, "lock_observed" lock_observed: flag, "initial_cap" initial_cap: dec,
         "final_cap" final_cap: dec, "accepted_cap_samples" accepted_cap_samples: dec);
    W0Track = 147, w0_track, "OER w0 track mac_prepared=true"
        ("tracking_due" tracking_due: flag, "tracking_us" tracking_us: wide,
         "mac_handshake_samples" mac_handshake_samples: dec,
         "mac_handshake_observations" mac_handshake_observations: dec);
    W0Temperature = 148, w0_temperature, "OER w0 temperature"
        ("value" value: signed, "acquisition" acquisition: name, "started_us" started_us: wide,
         "completed_us" completed_us: wide);
    W0Leave = 149, w0_leave, "OER w0 leave"
        ("domain" domain: name, "cold" cold: flag);
    W0ReleaseRefused = 150, w0_release_refused, "OER w0 release refused"
        ("error" error: name, "state" state: dec);
    W0JoinFailed = 151, w0_join_failed, "OER w0 join failed"
        ("cause" cause: name);
    W0TrackFailed = 155, w0_track_failed, "OER w0 track failed"
        ("cause" cause: name);
    W0Fail = 153, w0_fail, "OER-RESULT: fail w0 "
        ("" reason: name);
    W0Pass = 154, w0_pass, "OER-RESULT: pass w0" ();
    /// W1 console block.
    W1ConfigA = 200, w1_config_a, "w1.config"
        ("channel" channel: dec, "applied_channel" applied_channel: dec, "stand" stand: mactail, "ssid_matches" ssid_matches: dec, "window_ms" window_ms: dec, "cpu_hz" cpu_hz: dec, "arena" arena: by);
    W1ConfigB = 201, w1_config_b, ""
        ("slots" slots: dec, "mask" mask: hexa10);
    W1BssA = 202, w1_bss_a, "w1.bss"
        ("idx" idx: dec, "who" who: name, "bssid" bssid: mactail, "bi_tu" bi_tu: dec, "ch" ch: dec, "beacons" beacons: dec, "dup" dup: dec, "probe_rsp" probe_rsp: dec);
    W1BssB = 203, w1_bss_b, ""
        ("rssi" rssi: srange, "no_rssi" no_rssi: dec, "gaps_late" gaps_late: dec, "max_gap_us" max_gap_us: dec, "missed" missed: dec);
    W1BssC = 204, w1_bss_c, ""
        ("lag_us" lag_us: range, "ssid_len" ssid_len: dec, "ssid_ok" ssid_ok: bit, "stamp_span_us" stamp_span_us: dash);
    W1BssD = 205, w1_bss_d, ""
        ("handoff_span_us" handoff_span_us: dash);
    W1Gap = 206, w1_gap, "w1.gap"
        ("stamp" stamp: dec, "gap_us" gap_us: dec, "missed" missed: dash);
    W1Gaps = 207, w1_gaps, "w1.gaps"
        ("dropped" dropped: dec);
    W1Tick = 208, w1_tick, "w1.tick"
        ("start" start: dec, "end" end: dec, "ok" ok: bit);
    W1RxA = 209, w1_rx_a, "w1.rx"
        ("units" units: dec, "bytes" bytes: dec, "mac_irqs" mac_irqs: dec, "power_irqs" power_irqs: dec, "unserviced_mac" unserviced_mac: dec, "unserviced_pwr" unserviced_pwr: dec, "doorbell_unmasks" doorbell_unmasks: dec, "ignored_unmasks" ignored_unmasks: dec);
    W1RxB = 210, w1_rx_b, ""
        ("chained" chained: dec, "oversized" oversized: dec, "empty" empty: dec, "too_long" too_long: dec, "overload" overload: dec, "admission_blocks" admission_blocks: dec, "stage_capacity_blocks" stage_capacity_blocks: dec, "service_passes" service_passes: dec);
    W1RxC = 211, w1_rx_c, ""
        ("service_errors" service_errors: dec, "capacity_drains" capacity_drains: dec, "frames_after_release" frames_after_release: dec);
    W1HoldA = 212, w1_hold_a, "w1.hold"
        ("cycle" cycle: dec, "fill_ms" fill_ms: dec, "reached" reached: dec, "slots" slots: dec, "full" full: bit, "reason" reason: name, "units" units: dec, "mac_irqs" mac_irqs: dec, "stage_capacity_blocks" stage_capacity_blocks: dec);
    W1HoldB = 213, w1_hold_b, ""
        ("admission_blocks" admission_blocks: dec, "end_released" end_released: dash, "end_detached" end_detached: dash, "end_staged" end_staged: dash);
    W1HoldC = 214, w1_hold_c, ""
        ("end_observed" end_observed: dash, "end_recycle_start" end_recycle_start: dash, "end_accepted_tail" end_accepted_tail: dash, "end_next" end_next: hexdash);
    W1HoldD = 215, w1_hold_d, ""
        ("end_last" end_last: hexdash);
    W1ExhaustA = 216, w1_exhaust_a, "w1.exhaust"
        ("cycle" cycle: dec, "snapshots" snapshots: dec, "blocked_us" blocked_us: dash, "next_low" next_low: hexdash, "next_upper" next_upper: bitdash);
    W1ExhaustB = 217, w1_exhaust_b, ""
        ("last_low" last_low: hexdash, "base" base: hexdash, "walker" walker: bitdash, "doorbell" doorbell: bitdash, "buffer_full" buffer_full: dash);
    W1ExhaustC = 218, w1_exhaust_c, ""
        ("ring_base" ring_base: hexdash, "recycle_start" recycle_start: dash, "accepted_tail" accepted_tail: dash, "observed" observed: dash);
    W1ExhaustD = 219, w1_exhaust_d, ""
        ("completed" completed: dash, "remaining" remaining: dash, "reload_pending" reload_pending: bitdash, "released" released: dash);
    W1ExhaustE = 220, w1_exhaust_e, ""
        ("detached" detached: dash, "staged" staged: dash, "next_after_doorbell" next_after_doorbell: hexdash, "last_after_doorbell" last_after_doorbell: hexdash);
    W1ExhaustF = 221, w1_exhaust_f, ""
        ("desc_hash_changed" desc_hash_changed: bitdash);
    W1OwnA = 222, w1_own_a, "w1.own"
        ("cycle" cycle: dec, "order" order: name, "held" held: dec, "owner_clear" owner_clear: dec, "first_drop" first_drop: dash, "hold_mismatch" hold_mismatch: dec, "dwell_mismatch" dwell_mismatch: dec, "release_mismatch" release_mismatch: dec);
    W1OwnB = 223, w1_own_b, ""
        ("first_mismatch" first_mismatch: indexdash, "first_after_release" first_after_release: indices, "order_breaks" order_breaks: dec, "chained" chained: dec);
    W1OwnC = 224, w1_own_c, ""
        ("oversized" oversized: dec);
    W1PrefixA = 225, w1_prefix_a, "w1.prefix"
        ("invalid" invalid: dec, "chain" chain: dec, "bounds" bounds: dec, "rx_failure" rx_failure: dec, "mic_failure" mic_failure: dec, "quarantined" quarantined: dec, "output_small" output_small: dec, "metadata" metadata: dec);
    W1PrefixB = 226, w1_prefix_b, ""
        ("ignored" ignored: dec, "unsupported" unsupported: dec, "strict_rxend_refusable" strict_rxend_refusable: dec, "strict_dump_refusable" strict_dump_refusable: dec, "admitted" admitted: dec);
    W1ReloadA = 227, w1_reload_a, "w1.reload"
        ("max_attempts" max_attempts: dec, "limit_us" limit_us: wide, "observations" observations: dec, "reload_rerings" reload_rerings: dec, "rering_failures" rering_failures: dec, "base_writes" base_writes: dec, "reserved_landings" reserved_landings: dec);
    W1ReloadB = 228, w1_reload_b, ""
        ("reserved_late_landings" reserved_late_landings: dec, "walker_on_detached" walker_on_detached: dec);
    W1ContinuationA = 229, w1_continuation_a, "w1.continuation"
        ("immediate" immediate: dec, "timed" timed: dec, "drained" drained: dec, "upper_layer_blocked_but_droppable" upper_layer_blocked_but_droppable: dec, "probe_pending" probe_pending: dec, "recycled_append_pending" recycled_append_pending: dec, "budget_exhausted" budget_exhausted: dec, "network_backpressured" network_backpressured: dec);
    W1ContinuationB = 230, w1_continuation_b, ""
        ("protocol_blocked_by_tx" protocol_blocked_by_tx: dec, "longest_streak" longest_streak: dec);
    W1Teardown = 231, w1_teardown, "w1.teardown"
        ("mask_confirm_us" mask_confirm_us: dec, "drain_us" drain_us: dec, "drain_polls" drain_polls: dec);
    W1Exit = 232, w1_exit, "w1.exit"
        ("cause" cause: name, "end" end: name);
    W1P2A = 233, w1_p2_a, "w1.p2"
        ("phase" phase: name, "swi2_max_us" swi2_max_us: wide, "swi2_p99_us" swi2_p99_us: wide, "actor_max_us" actor_max_us: wide, "actor_p99_us" actor_p99_us: wide);
    W1P2B = 234, w1_p2_b, ""
        ("actor_over_bound" actor_over_bound: over, "receive_max_us" receive_max_us: wide, "receive_p99_us" receive_p99_us: wide);
    W1P2C = 235, w1_p2_c, ""
        ("mac_to_poll_max_us" mac_to_poll_max_us: wide, "ring_to_doorbell_max_us" ring_to_doorbell_max_us: wide);
    W1Clock = 236, w1_clock, "w1.clock"
        ("op" op: name, "clock" clock: name, "phase" phase: name, "count" count: dec, "max_us" max_us: wide);
    W1Stack = 237, w1_stack, "w1.stack"
        ("irq_bytes" irq_bytes: dec, "irq_free" irq_free: dec);
    W1TimerA = 238, w1_timer_a, "w1.timer"
        ("cmp" cmp: dec, "phase" phase: name, "early" early: dec, "late_count" late_count: dec, "late_max_us" late_max_us: wide, "late_total_us" late_total_us: wide);
    W1TimerB = 239, w1_timer_b, ""
        ("missed" missed: dec, "unmatched" unmatched: dec, "coalesced" coalesced: dec, "overlapping" overlapping: dec, "armed_at_end" armed_at_end: bit, "waits" waits: dec, "woke_early" woke_early: dec);
    W1Wake = 240, w1_wake, "w1.wake"
        ("cmp" cmp: dec, "phase" phase: name, "waits" waits: dec, "woke_early" woke_early: dec);
    W1Lag = 241, w1_lag, "w1.lag"
        ("i" i: dec, "stamp" stamp: dec, "lag_us" lag_us: dec, "since_prev_us" since_prev_us: dash, "until_next_us" until_next_us: dash);
    W1Lagfit = 242, w1_lagfit, "w1.lagfit"
        ("n" n: dec, "median_us" median_us: dash, "max_us" max_us: dash, "next_arrival_matches" next_arrival_matches: dec, "tolerance_us" tolerance_us: dec);
    W1Tsf = 243, w1_tsf, "w1.tsf"
        ("i" i: dec, "tsf" tsf: dec, "handoff" handoff: dec);
    W1TsffitA = 244, w1_tsffit_a, "w1.tsffit"
        ("n" n: dec, "bss" bss: dash, "median_us" median_us: dash, "p90_us" p90_us: dash);
    W1TsffitB = 245, w1_tsffit_b, ""
        ("max_us" max_us: dash, "slope_ppm" slope_ppm: sdash);
    W1Survey = 246, w1_survey, "w1.survey"
        ("frames" frames: dec, "header_refusals" header_refusals: dec, "data" data: dec, "ignored_management" ignored_management: dec, "table_full" table_full: dec, "off_channel" off_channel: dec);
    W1Verdict = 247, w1_verdict, "w1.verdict"
        ("beacons" beacons: wide, "bi_us" bi_us: wide, "window_us" window_us: wide, "required" required: wide);
    W1Begin = 248, w1_begin, "OER-W1-BEGIN v=1" ();
    W1End = 249, w1_end, "OER-W1-END"
        ("lines" lines: dec);
    W1DiscardsNone = 250, w1_discards_none, " discards=-" ();
    W1DiscardsA = 251, w1_discards_a, ""
        ("discards" counts: discards_lo);
    W1DiscardsB = 252, w1_discards_b, ""
        ("" counts: discards_hi);
    W1GateName = 253, w1_gate_name, "w1.gate"
        ("name" name: name);
    W1GatePass = 254, w1_gate_pass, " result=pass" ();
    W1GateFail = 255, w1_gate_fail, " result=fail"
        ("reason" reason: name);
    W1GateUnexercised = 256, w1_gate_unexercised, " result=unexercised"
        ("reason" reason: name);
    W1Pass = 257, w1_pass, "OER-RESULT: pass w1"
        ("beacons" beacons: wide, "required" required: wide);
    W1FailBegin = 258, w1_fail_begin, "OER-RESULT: fail w1" ();
    W1FailSeparator = 259, w1_fail_separator, ";" ();
    W1FailGate = 260, w1_fail_gate, " "
        ("" gate: phrase);
    W1FailReason = 261, w1_fail_reason, " "
        ("" reason: name);
    W1FailNotExercised = 262, w1_fail_not_exercised, " not exercised" ();
    W1FailEnd = 263, w1_fail_end, "" ();
    W1DoorbellFail = 265, w1_doorbell_fail, "OER-RESULT: fail w1 doorbell="
        ("" error: name, "source" source: dec, "first" first: dec, "second" second: dec);
    W1Fail = 266, w1_fail, "OER-RESULT: fail w1 "
        ("" reason: name);
    W1JoinFailed = 267, w1_join_failed, "OER w1 join failed"
        ("cause" cause: name);
    W1TrackFailed = 268, w1_track_failed, "OER w1 track failed"
        ("cause" cause: name);
    W1BindRefused = 269, w1_bind_refused, "OER w1 bind refused"
        ("error" error: name);
    W1BindClaimed = 270, w1_bind_claimed, "OER w1 bind refused: the arena was claimed" ();
    W1Start = 271, w1_start, "OER w1 start"
        ("epoch" epoch: dec, "calibration_path" calibration_path: name, "registration_us" registration_us: wide);
    W1StartPhy = 272, w1_start_phy, "OER w1 start"
        ("epoch" epoch: dec, "phy" phy: name);
    W1StartTrack = 273, w1_start_track, ""
        ("tracking_us" tracking_us: dash, "cold_mask" cold_mask: name, "expected_mask" expected_mask: hexa10);
    W1Irq = 274, w1_irq, "OER w1 irq"
        ("spurious_mac" spurious_mac: dec, "auxiliary_mac" auxiliary_mac: dec, "unhandled_mac" unhandled_mac: dec, "closed" closed: dec);
    W1NoFrame = 275, w1_no_frame, "OER w1 no frame within 5 s: the window did not open" ();
    W1ExitUnexpected = 276, w1_exit_unexpected, "OER w1 exit"
        ("cause" cause: name, "end" end: name);
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
    /// Diagnostic image only: inclusive public pilot service, corrected by
    /// that event's empty cycle-read pair; not the private SAR engine atom.
    PilotService = 78, pilot_service, "OPEN_RADIO_HIL pilot-service"
        ("tick" tick: dec, "event" event: dec, "entry-to-call-cycles" entry: wide,
         "service-cycles" service: wide, "empty-pair-cycles" cost: wide);
    PilotWindow = 79, pilot_window, "OPEN_RADIO_HIL pilot-window"
        ("tick" tick: dec, "events" events: dec, "lost" lost: dec, "cpu-hz" hz: dec,
         "capacity" capacity: dec);
}

impl Topic {
    /// How the topic's text joins its console line.
    pub const fn part(self) -> Part {
        match self {
            Self::W2ChannelFirst => Part::First,
            Self::W2ChannelLast => Part::Last,
            Self::W1StartTrack => Part::Last,
            Self::W1Start | Self::W1StartPhy => Part::First,
            Self::W1ConfigA => Part::First,
            Self::W1ConfigB => Part::Last,
            Self::W1BssA => Part::First,
            Self::W1BssB => Part::Middle,
            Self::W1BssC => Part::Middle,
            Self::W1BssD => Part::Last,
            Self::W1RxA => Part::First,
            Self::W1RxB => Part::Middle,
            Self::W1RxC => Part::Last,
            Self::W1HoldA => Part::First,
            Self::W1HoldB => Part::Middle,
            Self::W1HoldC => Part::Middle,
            Self::W1HoldD => Part::Last,
            Self::W1ExhaustA => Part::First,
            Self::W1ExhaustB => Part::Middle,
            Self::W1ExhaustC => Part::Middle,
            Self::W1ExhaustD => Part::Middle,
            Self::W1ExhaustE => Part::Middle,
            Self::W1ExhaustF => Part::Last,
            Self::W1OwnA => Part::First,
            Self::W1OwnB => Part::Middle,
            Self::W1OwnC => Part::Middle,
            Self::W1PrefixA => Part::First,
            Self::W1PrefixB => Part::Last,
            Self::W1ReloadA => Part::First,
            Self::W1ReloadB => Part::Last,
            Self::W1ContinuationA => Part::First,
            Self::W1ContinuationB => Part::Last,
            Self::W1P2A => Part::First,
            Self::W1P2B => Part::Middle,
            Self::W1P2C => Part::Last,
            Self::W1TimerA => Part::First,
            Self::W1TimerB => Part::Last,
            Self::W1TsffitA => Part::First,
            Self::W1TsffitB => Part::Last,
            Self::W1DiscardsNone => Part::Last,
            Self::W1DiscardsA => Part::Middle,
            Self::W1DiscardsB => Part::Last,
            Self::W1GateName => Part::First,
            Self::W1GatePass => Part::Last,
            Self::W1GateFail => Part::Last,
            Self::W1GateUnexercised => Part::Last,
            Self::W1FailBegin => Part::First,
            Self::W1FailSeparator => Part::Middle,
            Self::W1FailGate => Part::Middle,
            Self::W1FailReason => Part::Middle,
            Self::W1FailNotExercised => Part::Middle,
            Self::W1FailEnd => Part::Last,
            _ => Part::Whole,
        }
    }
}

#[cfg(test)]
mod tests;
