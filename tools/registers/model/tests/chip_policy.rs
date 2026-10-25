//! The ownership policy over a chip root on synthetic fixtures: one policy
//! that uses every construct, and one rejection per load-time rule.

use std::{error::Error as StdError, fs};

use oer_registers_model::{
    RegisterEvidenceSet,
    chip_model::ChipModel,
    chip_policy::{ChipPolicy, Claim, Error, ErrorKind, Holder, PartId},
};

type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

const EVIDENCE: &str = r#"
schema = 1

[[sources]]
id = "SRC_HEADER"
description = "Synthetic register header."

[[sources]]
id = "SRC_DRIVER"
description = "Synthetic driver."
"#;

const TIMER: &str = r#"
schema = 3

[type]
name = "TIMER"
description = "Synthetic timer."
sources = ["SRC_HEADER"]

[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[[registers]]
name = "CONFIG"
offset = 0x00

[[registers.fields]]
name = "DIV"
bits = "15:0"
class = "plain"

[[registers.fields]]
name = "EN"
bits = "31"
class = "plain"

[[registers]]
name = "INT_ENA"
offset = 0x04

[[registers.fields]]
name = "T0"
bits = "0"
class = "plain"

[[registers.fields]]
name = "T1"
bits = "1"
class = "plain"

[[registers]]
name = "INT_RAW"
offset = 0x08

[[registers.fields]]
name = "T0"
bits = "0"
class = "read-only"

[[registers.fields]]
name = "T1"
bits = "1"
class = "read-only"

[[registers]]
name = "INT_CLR"
offset = 0x0c

[[registers.fields]]
name = "T0"
bits = "0"
class = "write-one-to-clear"

[[registers.fields]]
name = "T1"
bits = "1"
class = "write-one-to-clear"

[[registers]]
name = "VALUE"
offset = 0x10

[[registers.fields]]
name = "VALUE"
bits = "31:0"
class = "read-only"

[[registers]]
name = "LOAD"
offset = 0x14

[[registers.fields]]
name = "LOAD"
bits = "0"
class = "write-trigger"

[[registers]]
name = "FIFO"
offset = 0x18
fifo = { sources = ["SRC_HEADER"] }

[[registers.fields]]
name = "DATA"
bits = "7:0"
class = "plain"

[[registers]]
name = "ALARM_%s"
offset = 0x20
dim = { count = 2, stride = 4 }

[[registers.fields]]
name = "ALARM"
bits = "31:0"
class = "plain"

[[registers]]
name = "ADDR"
offset = 0x28

[[registers.fields]]
name = "ADDR"
bits = "31:0"
class = "plain"

[[registers]]
name = "STATE"
offset = 0x2c

[[registers.fields]]
name = "FLAG"
bits = "0"
class = "read-clears"

[[registers]]
name = "WO"
offset = 0x30

[[registers.fields]]
name = "KEY"
bits = "7:0"
class = "write-only"
"#;

const ETM: &str = r#"
schema = 3

[type]
name = "ETM"
description = "Synthetic event-task matrix."
sources = ["SRC_HEADER"]

[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[[registers]]
name = "CHANNEL_ENABLE"
offset = 0x00

[[registers.fields]]
name = "CH0"
bits = "0"
class = "read-only"

[[registers.fields]]
name = "CH1"
bits = "1"
class = "read-only"

[[registers.fields]]
name = "CH2"
bits = "2"
class = "read-only"

[[registers.fields]]
name = "CH3"
bits = "3"
class = "read-only"

[[registers]]
name = "CHANNEL_ENABLE_SET"
offset = 0x04

[[registers.fields]]
name = "CH0"
bits = "0"
class = "write-trigger"

[[registers.fields]]
name = "CH1"
bits = "1"
class = "write-trigger"

[[registers.fields]]
name = "CH2"
bits = "2"
class = "write-trigger"

[[registers.fields]]
name = "CH3"
bits = "3"
class = "write-trigger"

[[registers]]
name = "CH%s_EVENT"
offset = 0x10
dim = { count = 4, stride = 4 }

[[registers.fields]]
name = "ID"
bits = "7:0"
class = "plain"
"#;

const COUNTER: &str = r#"
schema = 3

[type]
name = "COUNTER"
description = "Synthetic free-running counter."
sources = ["SRC_HEADER"]

[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[[registers]]
name = "COUNT"
offset = 0x00

[[registers.fields]]
name = "COUNT"
bits = "31:0"
class = "read-only"

[[registers]]
name = "UPDATE"
offset = 0x04

[[registers.fields]]
name = "UPDATE"
bits = "0"
class = "write-trigger"

[[registers]]
name = "MODE"
offset = 0x08

[[registers.fields]]
name = "STEP"
bits = "3:0"
class = "plain"

[[registers.fields]]
name = "AUTO"
bits = "4"
class = "plain"
"#;

const PLL: &str = r#"
schema = 3

[type]
name = "PLL"
description = "Synthetic analog PLL."
space = "analog-i2c"
sources = ["SRC_HEADER"]

[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[[registers]]
name = "OC_DIV"
offset = 0x02

[[registers.fields]]
name = "DIV"
bits = "5:0"
class = "plain"
"#;

const LAYOUT: &str = r#"
schema = 1

[layout]
name = "RX_DESCRIPTOR"
description = "Synthetic DMA descriptor."
size = 8
align = 4

[layout.review]
sources = ["SRC_HEADER"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[layout.words]]
name = "CONTROL"
offset = 0x00
access = "shared"

[[layout.words.fields]]
name = "OWNER"
bits = "31"
writer = "cpu"
description = "Who holds the descriptor."

[[layout.words]]
name = "BUFFER"
offset = 0x04
access = "shared"

[[layout.words.fields]]
name = "ADDRESS"
bits = "31:0"
writer = "cpu"
description = "The buffer's address."
placement = ["dma-window"]
"#;

const ROOT: &str = r#"
schema = 4
chip = "testchip"
interrupt-sources = { count = 0, sources = ["SRC_HEADER"] }
types = ["types/timer.toml", "types/etm.toml", "types/counter.toml", "types/pll.toml"]
layouts = ["layouts/rx-descriptor.toml"]

[[instances]]
name = "TIMER0"
type = "TIMER"
base = 0x60000000
extent = { size = 0x1000 }
sources = ["SRC_HEADER"]

[[instances]]
name = "TIMER1"
type = "TIMER"
base = 0x60001000
extent = { size = 0x1000 }
sources = ["SRC_HEADER"]

[[instances]]
name = "TIMER2"
type = "TIMER"
base = 0x60002000
extent = { size = 0x1000 }
sources = ["SRC_HEADER"]

[[instances]]
name = "ETM"
type = "ETM"
base = 0x60003000
extent = { size = 0x1000 }
sources = ["SRC_HEADER"]

[[instances]]
name = "COUNTER"
type = "COUNTER"
base = 0x60004000
extent = { size = 0x1000 }
sources = ["SRC_HEADER"]

[[analog-i2c.instances]]
name = "PLL"
type = "PLL"
code = 0x66
sources = ["SRC_HEADER"]

[[ram-regions]]
name = "dma-window"
base = 0x40800000
size = 0x10000
sources = ["SRC_HEADER"]

[[layout-placements]]
layout = "RX_DESCRIPTOR"
regions = ["dma-window"]
"#;

const POLICY: &str = r#"
schema = 6
chip = "testchip"

[[partitions]]
name = "TimerPeripherals"
member = "timer"
description = "Timer 0, its interrupt handler and the event channels it routes."

[[partitions.parts]]
name = "Task"
instance = "TIMER0"
registers = ["CONFIG", "VALUE", "LOAD", "FIFO", "ALARM_%s", "ADDR", "STATE", "WO"]

[[partitions.parts]]
name = "Irq"
instance = "TIMER0"
registers = ["INT_ENA", "INT_RAW", "INT_CLR"]
observes = ["VALUE"]
irq-side = true
description = "The handler samples INT_RAW, acknowledges it in INT_CLR and reads VALUE."
sources = ["SRC_DRIVER"]

[[partitions.parts]]
name = "Etm"
instance = "ETM"
registers = ["CHANNEL_ENABLE"]
elements = [{ register = "CH%s_EVENT", first = 0, count = 2 }]
triggers = [{ register = "CHANNEL_ENABLE_SET", fields = ["CH0", "CH1"] }]
description = "Event channels zero and one."
sources = ["SRC_DRIVER"]

[[partitions]]
name = "RadioPeripherals"
member = "radio"
description = "Event channel two and the counter of timer 1."

[[partitions.parts]]
name = "Etm"
instance = "ETM"
elements = [{ register = "CH%s_EVENT", first = 2, count = 1 }]
triggers = [{ register = "CHANNEL_ENABLE_SET", fields = ["CH2"] }]
observes = ["CHANNEL_ENABLE"]
description = "Event channel two."
sources = ["SRC_DRIVER"]

[[partitions.parts]]
name = "Value"
instance = "TIMER1"
registers = ["VALUE"]
read-only = true
copy = true
description = "Timer 1's counter, read from every radio context."
sources = ["SRC_DRIVER"]

[[partitions]]
name = "TimePeripherals"
member = "time"
description = "The free-running counter every context reads."

[[partitions.parts]]
name = "Counter"
instance = "COUNTER"
registers = ["COUNT", "UPDATE"]
shared = true
description = "Read and latched from every context."
sources = ["SRC_DRIVER"]

[[partitions.parts]]
name = "Mode"
instance = "COUNTER"
registers = ["MODE"]

[[unclaimed]]
instance = "TIMER1"
registers = ["CONFIG", "INT_ENA", "INT_RAW", "INT_CLR", "LOAD", "FIFO", "ALARM_%s", "ADDR", "STATE", "WO"]
reason = "Timer 1 has no driver; the radio reads its counter."

[[unclaimed]]
instance = "ETM"
registers = ["CH3_EVENT"]
triggers = [{ register = "CHANNEL_ENABLE_SET", fields = ["CH3"] }]
reason = "Event channel three is unused."

[[unclaimed]]
instances = ["TIMER2"]
reason = "Timer 2 has no driver."

[[flag-domains]]
name = "Channels"
description = "Event channels."
values = [{ name = "CH0", value = 1, description = "Channel zero.", sources = ["SRC_HEADER"] }]

[[enum-domains]]
name = "Event"
description = "Event identifiers."
values = [{ name = "Start", value = 1, description = "Start.", sources = ["SRC_HEADER"] }]

[[bounded-domains]]
name = "Divider"
description = "A clock divider."
min = 0
max = 0xffff
sources = ["SRC_HEADER"]

[[bounded-domains]]
name = "VendorArgument"
description = "A vendor routine's argument, truncated as the routine does."
min = 0
max = 0xffffffff
sources = ["SRC_HEADER"]

[[bounded-domains]]
name = "BitIndex"
description = "A bit of the divider."
min = 0
max = 15
sources = ["SRC_HEADER"]

[[opaque-domains]]
name = "AlarmImage"
description = "An alarm value."
register = "TIMER0.ALARM_%s"
sources = ["SRC_HEADER"]

[[opaque-domains]]
name = "ValueImage"
description = "Timer 1's counter."
register = "TIMER1.VALUE"
sources = ["SRC_HEADER"]

[[bitwise-composed-domains]]
name = "ConfigImage"
description = "A divider composed into the configuration word."
register = "TIMER0.CONFIG"
arguments = [{ name = "divider", value-type = "u16", left-shift = 0 }]
output-mask = 0x0000ffff
sources = ["SRC_HEADER"]

[[interrupt-snapshots]]
name = "timer0_interrupt"
status = "TIMER0.INT_RAW"
clear = "TIMER0.INT_CLR"
sources = ["SRC_DRIVER"]

[[field-reads]]
name = "read_timer0_value"
field = "TIMER0.VALUE.VALUE"
sources = ["SRC_DRIVER"]

[[full-register-reads]]
name = "observe_timer1_value"
field = "TIMER1.VALUE.VALUE"
domain = "ValueImage"
sources = ["SRC_DRIVER"]

[[partition-image-reads]]
name = "radio_image"
partition = "RadioPeripherals"
sources = ["SRC_DRIVER"]

[[pointer-fields]]
name = "timer0_descriptor"
field = "TIMER0.ADDR.ADDR"
points-to = "RX_DESCRIPTOR"
sources = ["SRC_DRIVER"]

[[full-register-writes]]
name = "write_timer0_alarm"
field = "TIMER0.ALARM_%s.ALARM"
domain = "AlarmImage"
sources = ["SRC_DRIVER"]

[[zero-based-field-writes]]
name = "write_timer0_key"
fields = ["TIMER0.WO.KEY"]
sources = ["SRC_DRIVER"]

[[sampled-bit-zero-writes]]
name = "republish_timer0_enable"
field = "TIMER0.INT_ENA.T0"
sources = ["SRC_DRIVER"]

[[zero-register-writes]]
name = "clear_timer0_config"
register = "TIMER0.CONFIG"
sources = ["SRC_DRIVER"]

[[fixed-register-images]]
name = "acknowledge_timer0_masked"
register = "TIMER0.INT_CLR"
value = 0x00000003
masks = "TIMER0.INT_ENA"
sources = ["SRC_DRIVER"]

[[fixed-register-images]]
name = "acknowledge_timer0_consumed"
register = "TIMER0.INT_CLR"
value = 0x00000001
consumed-event = "The caller reloads the timer itself."
sources = ["SRC_DRIVER"]

[[fixed-register-images]]
name = "enable_channel0"
register = "ETM.CHANNEL_ENABLE_SET"
value = 0x00000001
sources = ["SRC_DRIVER"]

[[fixed-register-images]]
name = "store_timer0_config_word"
register = "TIMER0.CONFIG"
value = 0x40000001
whole-word = true
sources = ["SRC_DRIVER"]

[[fixed-register-sequences]]
name = "route_channel2"
steps = [
    { register = "ETM.CH2_EVENT", value = 8 },
    { register = "ETM.CHANNEL_ENABLE_SET", value = 0x00000004 },
]
sources = ["SRC_DRIVER"]

[[masked-register-modifies]]
name = "set_timer0_divider"
register = "TIMER0.CONFIG"
domain = "ConfigImage"
preserve-mask = 0xffff0000
input-mask = 0x0000ffff
set-mask = 0
sources = ["SRC_DRIVER"]

[[field-or-modifies]]
name = "enable_timer0"
fields = [{ field = "TIMER0.CONFIG.EN", source-bit-offset = 0 }]
value = 1
sources = ["SRC_DRIVER"]

[[field-or-modifies]]
name = "point_timer0_descriptor"
fields = [{ field = "TIMER0.ADDR.ADDR", source-bit-offset = 0 }]
sources = ["SRC_DRIVER"]

[[field-clear-modifies]]
name = "disable_timer0"
fields = [{ field = "TIMER0.CONFIG.EN", source-bit-offset = 0 }]
value = 1
sources = ["SRC_DRIVER"]

[[field-replace-modifies]]
name = "replace_timer0_divider"
fields = [{ field = "TIMER0.CONFIG.DIV", source-bit-offset = 0 }]
domain = "Divider"
sources = ["SRC_DRIVER"]

[[field-replace-modifies]]
name = "replace_timer0_divider_from_argument"
fields = [{ field = "TIMER0.CONFIG.DIV", source-bit-offset = 0 }]
domain = "VendorArgument"
input-transform = { wrapping-subtract = 1, retain-mask = 0xffff }
sources = ["SRC_DRIVER"]

[[indexed-bit-set-modifies]]
name = "set_timer0_divider_bit"
field = "TIMER0.CONFIG.DIV"
domain = "BitIndex"
sources = ["SRC_DRIVER"]

[[field-reads]]
name = "read_channel_enable"
field = "ETM.CHANNEL_ENABLE.CH0"
sources = ["SRC_DRIVER"]
"#;

/// One fixture chip and its policy in their own temporary directory.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let fixture = Self {
            dir: tempfile::tempdir()?,
        };
        for (path, text) in [
            ("evidence.toml", EVIDENCE),
            ("root.toml", ROOT),
            ("types/timer.toml", TIMER),
            ("types/etm.toml", ETM),
            ("types/counter.toml", COUNTER),
            ("types/pll.toml", PLL),
            ("layouts/rx-descriptor.toml", LAYOUT),
            ("policy.toml", POLICY),
        ] {
            fixture.write(path, text)?;
        }
        Ok(fixture)
    }

    fn write(&self, path: &str, text: &str) -> TestResult {
        let path = self.dir.path().join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
        Ok(())
    }

    /// Replace the first `from` in `path` with `to`; `from` must be there.
    fn edit(&self, path: &str, from: &str, to: &str) -> TestResult<&Self> {
        let full = self.dir.path().join(path);
        let text = fs::read_to_string(&full)?;
        if !text.contains(from) {
            return Err(format!("{path} has no {from:?}").into());
        }
        fs::write(full, text.replacen(from, to, 1))?;
        Ok(self)
    }

    /// Append `text` to the policy.
    fn append(&self, text: &str) -> TestResult<&Self> {
        let path = self.dir.path().join("policy.toml");
        let mut policy = fs::read_to_string(&path)?;
        policy.push_str(text);
        fs::write(path, policy)?;
        Ok(self)
    }

    /// Remove the transaction `name` of `kind` from the policy.
    fn remove(&self, kind: &str, name: &str) -> TestResult<&Self> {
        let path = self.dir.path().join("policy.toml");
        let mut policy = fs::read_to_string(&path)?;
        let marker = format!("[[{kind}]]\nname = \"{name}\"\n");
        let start = policy
            .find(&marker)
            .ok_or_else(|| format!("the policy has no {kind} {name}"))?;
        let rest = start + marker.len();
        let end = policy
            .get(rest..)
            .and_then(|tail| tail.find("\n[["))
            .map_or(policy.len(), |offset| rest + offset + 1);
        policy.replace_range(start..end, "");
        fs::write(path, policy)?;
        Ok(self)
    }

    fn load(&self) -> TestResult<ChipPolicy> {
        let evidence = RegisterEvidenceSet::load_all(&[self.dir.path().join("evidence.toml")])?;
        let model = ChipModel::load(&self.dir.path().join("root.toml"), &evidence)?;
        Ok(ChipPolicy::load(
            &self.dir.path().join("policy.toml"),
            &model,
            &evidence,
        )?)
    }

    fn rejection(&self) -> TestResult<Error> {
        let evidence = RegisterEvidenceSet::load_all(&[self.dir.path().join("evidence.toml")])?;
        let model = ChipModel::load(&self.dir.path().join("root.toml"), &evidence)?;
        match ChipPolicy::load(&self.dir.path().join("policy.toml"), &model, &evidence) {
            Ok(_) => Err("the policy loaded".into()),
            Err(error) => Ok(error),
        }
    }
}

const fn part(partition: usize, part: usize) -> Holder {
    Holder::Part(PartId { partition, part })
}

#[test]
fn every_word_has_one_holder() -> TestResult {
    let policy = Fixture::new()?.load()?;
    assert_eq!(
        policy.claim("TIMER0.CONFIG"),
        Some(&Claim::Whole(part(0, 0)))
    );
    assert_eq!(
        policy.claim("TIMER0.INT_CLR"),
        Some(&Claim::Whole(part(0, 1)))
    );
    assert_eq!(
        policy.claim("ETM.CH1_EVENT"),
        Some(&Claim::Whole(part(0, 2)))
    );
    assert_eq!(
        policy.claim("ETM.CH2_EVENT"),
        Some(&Claim::Whole(part(1, 0)))
    );
    assert_eq!(
        policy.claim("ETM.CH3_EVENT"),
        Some(&Claim::Whole(Holder::Unclaimed(1)))
    );
    assert_eq!(
        policy.claim("TIMER1.VALUE"),
        Some(&Claim::Whole(part(1, 1)))
    );
    assert_eq!(
        policy.claim("TIMER1.ALARM_1"),
        Some(&Claim::Whole(Holder::Unclaimed(0)))
    );
    assert_eq!(
        policy.claim("TIMER2.WO"),
        Some(&Claim::Whole(Holder::Unclaimed(2)))
    );
    assert_eq!(
        policy.claim("COUNTER.COUNT"),
        Some(&Claim::Whole(part(2, 0)))
    );
    let Some(Claim::Fields(fields)) = policy.claim("ETM.CHANNEL_ENABLE_SET") else {
        return Err("the trigger word is not split".into());
    };
    let expected = [
        ("CH0", part(0, 2)),
        ("CH1", part(0, 2)),
        ("CH2", part(1, 0)),
        ("CH3", Holder::Unclaimed(1)),
    ];
    assert_eq!(
        fields
            .iter()
            .map(|(name, holder)| (name.as_str(), *holder))
            .collect::<Vec<_>>(),
        expected
    );
    // Analog-I2C bytes are reached through their master, not partitioned.
    assert_eq!(policy.claim("PLL.OC_DIV"), None);
    assert_eq!(policy.claims().len(), 3 * 12 + 6 + 3);
    Ok(())
}

#[test]
fn every_transaction_has_one_part() -> TestResult {
    let policy = Fixture::new()?.load()?;
    let owner = |name: &str| policy.owner(name).map(Holder::Part);
    assert_eq!(owner("timer0_interrupt"), Some(part(0, 1)));
    assert_eq!(owner("acknowledge_timer0_masked"), Some(part(0, 1)));
    assert_eq!(owner("write_timer0_alarm"), Some(part(0, 0)));
    assert_eq!(owner("enable_channel0"), Some(part(0, 2)));
    assert_eq!(owner("route_channel2"), Some(part(1, 0)));
    assert_eq!(owner("observe_timer1_value"), Some(part(1, 1)));
    assert_eq!(owner("radio_image"), None);
    let parts = policy
        .partitions()
        .iter()
        .map(|partition| partition.parts.len())
        .collect::<Vec<_>>();
    assert_eq!(parts, [3, 2, 2]);
    assert!(
        policy
            .part(PartId {
                partition: 1,
                part: 1
            })
            .is_some_and(|part| part.copy)
    );
    Ok(())
}

#[test]
fn errors_name_the_file_and_the_entity() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit("policy.toml", "\"CH3_EVENT\"", "\"CH2_EVENT\"")?;
    let error = fixture.rejection()?;
    assert!(error.path.ends_with("policy.toml"), "{error}");
    assert_eq!(error.entity, "ETM.CH2_EVENT");
    assert!(
        matches!(error.kind, ErrorKind::ClaimedTwice(ref other) if other == "part RadioPeripherals.Etm"),
        "{error}"
    );
    Ok(())
}

/// The fixture after `edits` to the policy, rejected: its error.
fn rejected(edits: &[(&str, &str)]) -> TestResult<Error> {
    let fixture = Fixture::new()?;
    for (from, to) in edits {
        fixture.edit("policy.toml", from, to)?;
    }
    fixture.rejection()
}

macro_rules! rejects {
    ($name:ident, [$(($from:expr, $to:expr)),+ $(,)?], $kind:pat) => {
        #[test]
        fn $name() -> TestResult {
            let error = rejected(&[$(($from, $to)),+])?;
            assert!(matches!(error.kind, $kind), "{error}");
            Ok(())
        }
    };
}

// The file.
rejects!(
    schema,
    [("schema = 6", "schema = 5")],
    ErrorKind::Schema { .. }
);
rejects!(
    chip,
    [("chip = \"testchip\"", "chip = \"otherchip\"")],
    ErrorKind::Chip { .. }
);
rejects!(
    handoff_is_gone,
    [("member = \"timer\"", "member = \"timer\"\nhandoff = true")],
    ErrorKind::Parse(_)
);

// Partitions and parts.
rejects!(
    partition_name,
    [(
        "name = \"TimerPeripherals\"",
        "name = \"timer_peripherals\""
    )],
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_partition,
    [("name = \"RadioPeripherals\"", "name = \"TimerPeripherals\"")],
    ErrorKind::Duplicate(_)
);
rejects!(
    duplicate_member,
    [("member = \"radio\"", "member = \"timer\"")],
    ErrorKind::Duplicate(_)
);
rejects!(
    empty_partition,
    [(
        "[[unclaimed]]",
        "[[partitions]]\nname = \"Empty\"\nmember = \"empty\"\ndescription = \"Nothing.\"\nparts = []\n\n[[unclaimed]]"
    )],
    ErrorKind::EmptyPartition
);
rejects!(
    part_name,
    [("name = \"Task\"", "name = \"task\"")],
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_part,
    [("name = \"Irq\"", "name = \"Task\"")],
    ErrorKind::Duplicate(_)
);
rejects!(
    unknown_instance,
    [(
        "instance = \"COUNTER\"\nregisters = [\"MODE\"]",
        "instance = \"TIMER9\"\nregisters = [\"MODE\"]"
    )],
    ErrorKind::UnknownInstance(_)
);
rejects!(
    analog_instance_is_not_partitioned,
    [(
        "instance = \"COUNTER\"\nregisters = [\"MODE\"]",
        "instance = \"PLL\"\nregisters = [\"OC_DIV\"]"
    )],
    ErrorKind::UnknownInstance(_)
);
rejects!(
    part_selects_nothing,
    [(
        "instance = \"COUNTER\"\nregisters = [\"MODE\"]",
        "instance = \"COUNTER\""
    )],
    ErrorKind::EmptySelection
);
rejects!(
    unknown_register,
    [("registers = [\"MODE\"]", "registers = [\"MODES\"]")],
    ErrorKind::UnknownRegister(_)
);
rejects!(
    element_range,
    [("first = 2, count = 1", "first = 2, count = 3")],
    ErrorKind::ElementRange(_)
);
rejects!(
    element_range_empty,
    [("first = 2, count = 1", "first = 2, count = 0")],
    ErrorKind::ElementRange(_)
);
rejects!(
    word_held_by_two_parts,
    [(
        "registers = [\"MODE\"]",
        "registers = [\"MODE\", \"COUNT\"]"
    )],
    ErrorKind::ClaimedTwice(_)
);
rejects!(
    word_held_and_unclaimed,
    [(
        "registers = [\"CH3_EVENT\"]",
        "registers = [\"CH3_EVENT\", \"CH0_EVENT\"]"
    )],
    ErrorKind::ClaimedTwice(_)
);
rejects!(
    word_uncovered,
    [
        ("registers = [\"MODE\"]", "registers = [\"COUNT\"]"),
        (
            "registers = [\"COUNT\", \"UPDATE\"]",
            "registers = [\"UPDATE\"]"
        )
    ],
    ErrorKind::Uncovered
);
rejects!(
    trigger_field_uncovered,
    [(
        "triggers = [{ register = \"CHANNEL_ENABLE_SET\", fields = [\"CH3\"] }]\n",
        ""
    )],
    ErrorKind::Uncovered
);
rejects!(
    trigger_field_held_twice,
    [("fields = [\"CH2\"]", "fields = [\"CH1\", \"CH2\"]")],
    ErrorKind::ClaimedTwice(_)
);
rejects!(
    triggers_need_trigger_words,
    [(
        "register = \"CHANNEL_ENABLE_SET\", fields = [\"CH2\"]",
        "register = \"CHANNEL_ENABLE\", fields = [\"CH2\"]"
    )],
    ErrorKind::Triggers(_)
);
rejects!(
    unknown_trigger_field,
    [("fields = [\"CH2\"]", "fields = [\"CH9\"]")],
    ErrorKind::UnknownField(_)
);

// Unclaimed words.
rejects!(
    unclaimed_form,
    [(
        "instances = [\"TIMER2\"]",
        "instances = [\"TIMER2\"]\ninstance = \"TIMER2\""
    )],
    ErrorKind::UnclaimedForm(_)
);
rejects!(
    unclaimed_reason,
    [("reason = \"Timer 2 has no driver.\"", "reason = \" \"")],
    ErrorKind::MissingDescription
);
rejects!(
    unclaimed_unknown_instance,
    [("instances = [\"TIMER2\"]", "instances = [\"TIMER9\"]")],
    ErrorKind::UnknownInstance(_)
);

// Flags.
rejects!(
    shared_irq_side,
    [("shared = true", "shared = true\nirq-side = true")],
    ErrorKind::Flags(_)
);
rejects!(
    copy_needs_read_only,
    [("read-only = true\ncopy = true", "copy = true")],
    ErrorKind::Flags(_)
);
rejects!(
    read_only_writes_no_trigger,
    [(
        "observes = [\"CHANNEL_ENABLE\"]",
        "observes = [\"CHANNEL_ENABLE\"]\nread-only = true"
    )],
    ErrorKind::Flags(_)
);
rejects!(
    flagged_part_cites_sources,
    [(
        "description = \"Event channel two.\"\nsources = [\"SRC_DRIVER\"]",
        "description = \"Event channel two.\""
    )],
    ErrorKind::InvalidSources
);
rejects!(
    unknown_source,
    [(
        "description = \"Event channel two.\"\nsources = [\"SRC_DRIVER\"]",
        "description = \"Event channel two.\"\nsources = [\"SRC_NONE\"]"
    )],
    ErrorKind::UnknownSource(_)
);
rejects!(
    observes_own_word,
    [("observes = [\"VALUE\"]", "observes = [\"INT_RAW\"]")],
    ErrorKind::Observes(_)
);
rejects!(
    observes_unclaimed_word,
    [(
        "observes = [\"CHANNEL_ENABLE\"]",
        "observes = [\"CH3_EVENT\"]"
    )],
    ErrorKind::Observes(_)
);
rejects!(
    observes_with_side_effect,
    [("observes = [\"VALUE\"]", "observes = [\"STATE\"]")],
    ErrorKind::Observes(_)
);
rejects!(
    read_only_part_reads_freely,
    [
        (
            "registers = [\"VALUE\"]\nread-only = true",
            "registers = [\"VALUE\", \"STATE\"]\nread-only = true"
        ),
        (
            "\"ADDR\", \"STATE\", \"WO\"]\nreason",
            "\"ADDR\", \"WO\"]\nreason"
        ),
    ],
    ErrorKind::ReadOnly(_)
);
#[test]
fn shared_part_rejects_a_word_of_two_written_fields() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "policy.toml",
            "registers = [\"COUNT\", \"UPDATE\"]",
            "registers = [\"COUNT\", \"UPDATE\", \"MODE\"]",
        )?
        .edit(
            "policy.toml",
            "[[partitions.parts]]\nname = \"Mode\"\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]\n",
            "",
        )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::Shared(_)), "{error}");
    Ok(())
}

// Domains.
rejects!(
    domain_name,
    [("name = \"Divider\"", "name = \"divider\"")],
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_domain,
    [("name = \"BitIndex\"", "name = \"Divider\"")],
    ErrorKind::Duplicate(_)
);
rejects!(
    flag_value_name,
    [("{ name = \"CH0\", value = 1", "{ name = \"Ch0\", value = 1")],
    ErrorKind::InvalidName(_)
);
rejects!(
    bounded_domain_order,
    [("min = 0\nmax = 15", "min = 16\nmax = 15")],
    ErrorKind::Domain(_)
);
rejects!(
    opaque_domain_register,
    [(
        "register = \"TIMER1.VALUE\"",
        "register = \"TIMER1.NOTHING\""
    )],
    ErrorKind::UnknownRegister(_)
);
rejects!(
    composed_domain_shift,
    [("left-shift = 0", "left-shift = 32")],
    ErrorKind::Domain(_)
);

// Transactions: names, references and domains.
rejects!(
    transaction_name,
    [("name = \"enable_timer0\"", "name = \"EnableTimer0\"")],
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_transaction,
    [("name = \"disable_timer0\"", "name = \"enable_timer0\"")],
    ErrorKind::Duplicate(_)
);
rejects!(
    transaction_sources,
    [(
        "name = \"enable_timer0\"\nfields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 1\nsources = [\"SRC_DRIVER\"]",
        "name = \"enable_timer0\"\nfields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 1\nsources = []"
    )],
    ErrorKind::InvalidSources
);
rejects!(
    unknown_field,
    [(
        "field = \"TIMER0.VALUE.VALUE\"",
        "field = \"TIMER0.VALUE.COUNT\""
    )],
    ErrorKind::UnknownField(_)
);
rejects!(
    unknown_transaction_register,
    [(
        "register = \"TIMER0.CONFIG\"\nsources",
        "register = \"TIMER0.CONFIGURATION\"\nsources"
    )],
    ErrorKind::UnknownRegister(_)
);
rejects!(
    unknown_domain,
    [("domain = \"Divider\"", "domain = \"Divisor\"")],
    ErrorKind::Domain(_)
);
rejects!(
    bound_domain_elsewhere,
    [("domain = \"AlarmImage\"", "domain = \"ValueImage\"")],
    ErrorKind::Domain(_)
);
rejects!(
    bit_index_domain_too_wide,
    [("domain = \"BitIndex\"", "domain = \"Divider\"")],
    ErrorKind::Domain(_)
);
rejects!(
    partition_image_of_unknown_partition,
    [(
        "partition = \"RadioPeripherals\"",
        "partition = \"ClockPeripherals\""
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    pointer_to_unknown_layout,
    [(
        "points-to = \"RX_DESCRIPTOR\"",
        "points-to = \"TX_DESCRIPTOR\""
    )],
    ErrorKind::Pointer(_)
);
rejects!(
    full_write_takes_one_value,
    [(
        "domain = \"AlarmImage\"",
        "domain = \"AlarmImage\"\nplacement = \"dma-window\""
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    modify_takes_domain_or_value,
    [(
        "value = 1\nsources = [\"SRC_DRIVER\"]\n\n[[field-or-modifies]]",
        "sources = [\"SRC_DRIVER\"]\n\n[[field-or-modifies]]"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    projections_of_one_register,
    [(
        "fields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 1",
        "fields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }, { field = \"TIMER0.ALARM_0.ALARM\", source-bit-offset = 1 }]\nvalue = 1"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    projection_input_bits,
    [(
        "{ field = \"TIMER0.CONFIG.DIV\", source-bit-offset = 0 }",
        "{ field = \"TIMER0.CONFIG.DIV\", source-bit-offset = 17 }"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    masks_are_disjoint,
    [("preserve-mask = 0xffff0000", "preserve-mask = 0xffff0001")],
    ErrorKind::Transaction(_)
);

// Transactions: field classes.
rejects!(
    read_of_write_only_field,
    [(
        "field = \"TIMER0.VALUE.VALUE\"",
        "field = \"TIMER0.WO.KEY\""
    )],
    ErrorKind::FieldClass(_)
);
rejects!(
    read_with_side_effect,
    [(
        "field = \"TIMER0.VALUE.VALUE\"",
        "field = \"TIMER0.STATE.FLAG\""
    )],
    ErrorKind::FieldClass(_)
);
rejects!(
    write_of_read_only_field,
    [(
        "fields = [\"TIMER0.WO.KEY\"]",
        "fields = [\"TIMER0.VALUE.VALUE\"]"
    )],
    ErrorKind::FieldClass(_)
);
rejects!(
    write_one_to_clear_by_another_transaction,
    [(
        "fields = [\"TIMER0.WO.KEY\"]",
        "fields = [\"TIMER0.INT_CLR.T0\"]"
    )],
    ErrorKind::WriteOneToClear(_)
);
rejects!(
    acknowledgement_needs_a_proof,
    [(
        "consumed-event = \"The caller reloads the timer itself.\"\n",
        ""
    )],
    ErrorKind::WriteOneToClear(_)
);
rejects!(
    proof_only_on_a_clear_word,
    [(
        "value = 0x40000001\nwhole-word = true",
        "value = 0x40000001\nwhole-word = true\nconsumed-event = \"None.\""
    )],
    ErrorKind::WriteOneToClear(_)
);
rejects!(
    sequence_writes_no_clear_word,
    [(
        "{ register = \"ETM.CH2_EVENT\", value = 8 },",
        "{ register = \"TIMER0.INT_CLR\", value = 1 },"
    )],
    ErrorKind::WriteOneToClear(_)
);
rejects!(
    undeclared_bits_need_whole_word,
    [(
        "value = 0x40000001\nwhole-word = true",
        "value = 0x40000001"
    )],
    ErrorKind::Register(_)
);
rejects!(
    image_sets_no_read_clears_bit,
    [(
        "register = \"TIMER0.CONFIG\"\nvalue = 0x40000001",
        "register = \"TIMER0.STATE\"\nvalue = 0x00000001"
    )],
    ErrorKind::FieldClass(_)
);
rejects!(
    modify_of_fifo,
    [(
        "fields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 1",
        "fields = [{ field = \"TIMER0.FIFO.DATA\", source-bit-offset = 0 }]\nvalue = 1"
    )],
    ErrorKind::Register(_)
);
rejects!(
    modify_of_read_only_field,
    [(
        "fields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 1",
        "fields = [{ field = \"TIMER0.VALUE.VALUE\", source-bit-offset = 0 }]\nvalue = 1"
    )],
    ErrorKind::FieldClass(_)
);
rejects!(
    store_of_fifo,
    [(
        "register = \"TIMER0.CONFIG\"\nsources",
        "register = \"TIMER0.FIFO\"\nsources"
    )],
    ErrorKind::Register(_)
);
rejects!(
    snapshot_clear_word_class,
    [("clear = \"TIMER0.INT_CLR\"", "clear = \"TIMER0.INT_ENA\"")],
    ErrorKind::Snapshot(_)
);
rejects!(
    snapshot_names_elements,
    [(
        "status = \"TIMER0.INT_RAW\"",
        "status = \"TIMER0.ALARM_%s\""
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    masks_names_an_enable_word,
    [("masks = \"TIMER0.INT_ENA\"", "masks = \"TIMER0.INT_RAW\"")],
    ErrorKind::WriteOneToClear(_)
);

// Transactions: ownership.
rejects!(
    transaction_on_unclaimed_word,
    [(
        "field = \"TIMER0.VALUE.VALUE\"",
        "field = \"TIMER1.CONFIG.EN\""
    )],
    ErrorKind::ReachesUnclaimed(_)
);
rejects!(
    transaction_on_unclaimed_template,
    [(
        "field = \"TIMER0.ALARM_%s.ALARM\"\ndomain = \"AlarmImage\"",
        "field = \"TIMER1.ALARM_%s.ALARM\"\ndomain = \"Divider\""
    )],
    ErrorKind::ReachesUnclaimed(_)
);
rejects!(
    transaction_on_split_template,
    [(
        "fields = [\"TIMER0.WO.KEY\"]",
        "fields = [\"ETM.CH%s_EVENT.ID\"]"
    )],
    ErrorKind::SeveralOwners(_)
);
rejects!(
    transaction_across_parts,
    [(
        "{ register = \"ETM.CH2_EVENT\", value = 8 },",
        "{ register = \"ETM.CH1_EVENT\", value = 8 },"
    )],
    ErrorKind::SeveralOwners(_)
);
rejects!(
    split_word_bits_of_two_parts,
    [(
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000001",
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000005"
    )],
    ErrorKind::SeveralOwners(_)
);
rejects!(
    split_word_unclaimed_bits,
    [(
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000001",
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000008"
    )],
    ErrorKind::ReachesUnclaimed(_)
);
rejects!(
    split_word_written_whole,
    [(
        "register = \"TIMER0.CONFIG\"\nsources",
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nsources"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    transaction_on_shared_part,
    [(
        "field = \"TIMER0.VALUE.VALUE\"",
        "field = \"COUNTER.COUNT.COUNT\""
    )],
    ErrorKind::Shared(_)
);
rejects!(
    write_by_read_only_part,
    [
        (
            "registers = [\"VALUE\"]\nread-only = true",
            "registers = [\"VALUE\", \"INT_ENA\"]\nread-only = true"
        ),
        (
            "registers = [\"CONFIG\", \"INT_ENA\", \"INT_RAW\"",
            "registers = [\"CONFIG\", \"INT_RAW\""
        ),
        (
            "register = \"TIMER0.CONFIG\"\nsources",
            "register = \"TIMER1.INT_ENA\"\nsources"
        ),
    ],
    ErrorKind::ReadOnly(_)
);
rejects!(
    snapshot_across_parts,
    [
        (
            "registers = [\"CONFIG\", \"VALUE\",",
            "registers = [\"CONFIG\", \"INT_RAW\", \"VALUE\","
        ),
        (
            "registers = [\"INT_ENA\", \"INT_RAW\", \"INT_CLR\"]",
            "registers = [\"INT_ENA\", \"INT_CLR\"]"
        ),
    ],
    ErrorKind::SeveralOwners(_)
);

// Regressions of the cross-model review: every guarantee the schema-5
// generator checks holds in this format.

/// The rule a loaded fixture breaks after `fixture`'s edits.
fn kind_of(fixture: &Fixture) -> TestResult<ErrorKind> {
    Ok(fixture.rejection()?.kind)
}

#[test]
fn snapshot_reads_an_observed_status_word() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "policy.toml",
            "\"CONFIG\", \"VALUE\", \"LOAD\"",
            "\"CONFIG\", \"INT_RAW\", \"VALUE\", \"LOAD\"",
        )?
        .edit(
            "policy.toml",
            "registers = [\"INT_ENA\", \"INT_RAW\", \"INT_CLR\"]",
            "registers = [\"INT_ENA\", \"INT_CLR\"]",
        )?
        .edit(
            "policy.toml",
            "observes = [\"VALUE\"]",
            "observes = [\"VALUE\", \"INT_RAW\"]",
        )?;
    let policy = fixture.load()?;
    assert_eq!(
        policy.owner("timer0_interrupt").map(Holder::Part),
        Some(part(0, 1))
    );
    Ok(())
}

#[test]
fn pointer_image_replaces_a_pointer_field() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "[[field-or-modifies]]\nname = \"point_timer0_descriptor\"",
        "[[field-replace-modifies]]\nname = \"point_timer0_descriptor\"",
    )?;
    fixture.load()?;
    Ok(())
}

#[test]
fn zero_replaces_a_pointer_field_with_null() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(
        "\n[[field-replace-modifies]]\nname = \"null_timer0_descriptor\"\nfields = [{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }]\nvalue = 0\nsources = [\"SRC_DRIVER\"]\n",
    )?;
    fixture.load()?;
    Ok(())
}

#[test]
fn enum_indexes_a_bit() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit("policy.toml", "domain = \"BitIndex\"", "domain = \"Event\"")?;
    fixture.load()?;
    Ok(())
}

#[test]
fn enum_index_lies_below_the_field_width() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit("policy.toml", "domain = \"BitIndex\"", "domain = \"Event\"")?
        .edit(
            "policy.toml",
            "value = 1, description = \"Start.\"",
            "value = 16, description = \"Start.\"",
        )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Domain(_)));
    Ok(())
}

const FORGE_IMAGE: &str = "\n[[fixed-register-images]]\nname = \"forge_pointer\"\nregister = \"TIMER0.ADDR\"\nvalue = 0xdeadbeef\nsources = [\"SRC_DRIVER\"]\n";
const FORGE_WRITE: &str = "\n[[full-register-writes]]\nname = \"forge_pointer\"\nfield = \"TIMER0.ADDR.ADDR\"\ndomain = \"Divider\"\nsources = [\"SRC_DRIVER\"]\n";

#[test]
fn integer_writers_cannot_forge_a_pointer() -> TestResult {
    for text in [FORGE_IMAGE, FORGE_WRITE] {
        let fixture = Fixture::new()?;
        fixture.append(text)?;
        assert!(
            matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)),
            "{text}"
        );
    }
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "name = \"point_timer0_descriptor\"\nfields = [{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }]",
        "name = \"point_timer0_descriptor\"\nfields = [{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }]\ndomain = \"Divider\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn pointer_encoding_fills_its_field() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "points-to = \"RX_DESCRIPTOR\"",
        "points-to = \"RX_DESCRIPTOR\"\nencoding = \"ble-compressed\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn pointer_targets_a_layout_in_the_dma_window() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "root.toml",
        "[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
        "",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn compressed_pointer_names_a_word() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"ADDR\"\nbits = \"31:0\"",
            "name = \"ADDR\"\nbits = \"19:0\"",
        )?
        .edit(
            "policy.toml",
            "points-to = \"RX_DESCRIPTOR\"",
            "points-to = \"RX_DESCRIPTOR\"\nencoding = \"ble-compressed\"\nmember-offset = 4",
        )?;
    fixture.load()?;
    fixture.edit("policy.toml", "member-offset = 4", "member-offset = 1")?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn buffer_address_lies_in_the_dma_window() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "domain = \"AlarmImage\"",
        "placement = \"dma-window\"",
    )?;
    fixture.load()?;
    fixture
        .edit("root.toml", "name = \"dma-window\"", "name = \"scratch\"")?
        .edit(
            "types/../layouts/rx-descriptor.toml",
            "placement = [\"dma-window\"]\n",
            "",
        )?
        .edit(
            "root.toml",
            "[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
            "",
        )?
        .remove("pointer-fields", "timer0_descriptor")?
        .remove("field-or-modifies", "point_timer0_descriptor")?
        .edit(
            "policy.toml",
            "placement = \"dma-window\"",
            "placement = \"scratch\"",
        )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn masked_modify_partitions_the_word() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "preserve-mask = 0xffff0000",
        "preserve-mask = 0",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)));
    Ok(())
}

#[test]
fn masked_modify_takes_every_bit_of_its_domain() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "preserve-mask = 0xffff0000\ninput-mask = 0x0000ffff",
        "preserve-mask = 0xfffffffe\ninput-mask = 1",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)));
    Ok(())
}

#[test]
fn masking_proof_masks_every_acknowledged_event() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"T1\"\nbits = \"1\"\nclass = \"plain\"",
        "name = \"T1\"\nbits = \"2\"\nclass = \"plain\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::WriteOneToClear(_)));
    Ok(())
}

#[test]
fn partition_image_reads_without_side_effects() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "partition = \"RadioPeripherals\"",
        "partition = \"TimerPeripherals\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Register(_)));
    Ok(())
}

#[test]
fn sampled_bit_reads_without_side_effects() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"LOAD\"\noffset = 0x14",
            "name = \"LOAD\"\noffset = 0x14\nread-side-effect = { sources = [\"SRC_HEADER\"] }",
        )?
        .edit("types/timer.toml", "class = \"write-trigger\"", "class = \"plain\"")?
        .append(
            "\n[[sampled-bit-zero-writes]]\nname = \"sample_load\"\nfield = \"TIMER0.LOAD.LOAD\"\nsources = [\"SRC_DRIVER\"]\n",
        )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Register(_)));
    Ok(())
}

#[test]
fn full_write_names_a_whole_word_field() -> TestResult {
    for field in ["TIMER0.CONFIG.DIV", "ETM.CHANNEL_ENABLE_SET.CH0"] {
        let fixture = Fixture::new()?;
        fixture.append(&format!(
            "\n[[full-register-writes]]\nname = \"partial_write\"\nfield = \"{field}\"\ndomain = \"Divider\"\nsources = [\"SRC_DRIVER\"]\n"
        ))?;
        assert!(
            matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn zero_write_needs_a_written_field() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(
        "\n[[zero-register-writes]]\nname = \"zero_counter\"\nregister = \"TIMER0.VALUE\"\nsources = [\"SRC_DRIVER\"]\n",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Register(_)));
    Ok(())
}

#[test]
fn zero_write_leaves_write_one_to_clear_words_alone() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .remove("interrupt-snapshots", "timer0_interrupt")?
        .remove("fixed-register-images", "acknowledge_timer0_masked")?
        .remove("fixed-register-images", "acknowledge_timer0_consumed")?
        .append(
            "\n[[zero-register-writes]]\nname = \"zero_clear\"\nregister = \"TIMER0.INT_CLR\"\nsources = [\"SRC_DRIVER\"]\n",
        )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::WriteOneToClear(_)));
    Ok(())
}

#[test]
fn snapshot_samples_only_readable_status_fields() -> TestResult {
    for class in ["write-only", "write-trigger"] {
        let fixture = Fixture::new()?;
        fixture.edit(
            "types/timer.toml",
            "name = \"INT_RAW\"\noffset = 0x08\n\n[[registers.fields]]\nname = \"T0\"\nbits = \"0\"\nclass = \"read-only\"",
            &format!("name = \"INT_RAW\"\noffset = 0x08\n\n[[registers.fields]]\nname = \"T0\"\nbits = \"0\"\nclass = \"{class}\""),
        )?;
        let error = fixture.rejection()?;
        assert_eq!(error.entity, "timer0_interrupt");
        assert!(matches!(error.kind, ErrorKind::Snapshot(_)), "{error}");
    }
    Ok(())
}

#[test]
fn every_fixed_write_needs_a_writable_word() -> TestResult {
    for (value, whole_word) in [(0u32, false), (0, true), (0xdeadbeef, true)] {
        for sequence in [false, true] {
            let fixture = Fixture::new()?;
            let operation = if sequence {
                format!(
                    "[[fixed-register-sequences]]\nname = \"write_counter\"\nsteps = [{{ register = \"TIMER0.VALUE\", value = {value}, whole-word = {whole_word} }}]"
                )
            } else {
                format!(
                    "[[fixed-register-images]]\nname = \"write_counter\"\nregister = \"TIMER0.VALUE\"\nvalue = {value}\nwhole-word = {whole_word}"
                )
            };
            fixture.append(&format!("\n{operation}\nsources = [\"SRC_DRIVER\"]\n"))?;
            let error = fixture.rejection()?;
            assert_eq!(error.entity, "write_counter");
            assert!(matches!(error.kind, ErrorKind::Register(_)), "{error}");
        }
    }
    Ok(())
}

#[test]
fn every_fixed_write_checks_w1c_without_a_snapshot() -> TestResult {
    for sequence in [false, true] {
        let fixture = Fixture::new()?;
        fixture
            .remove("interrupt-snapshots", "timer0_interrupt")?
            .remove("fixed-register-images", "acknowledge_timer0_masked")?
            .remove("fixed-register-images", "acknowledge_timer0_consumed")?;
        let operation = if sequence {
            "[[fixed-register-sequences]]\nname = \"write_clear\"\nsteps = [{ register = \"TIMER0.INT_CLR\", value = 0 }]"
        } else {
            "[[fixed-register-images]]\nname = \"write_clear\"\nregister = \"TIMER0.INT_CLR\"\nvalue = 0"
        };
        fixture.append(&format!("\n{operation}\nsources = [\"SRC_DRIVER\"]\n"))?;
        let error = fixture.rejection()?;
        assert_eq!(error.entity, "write_clear");
        assert!(
            matches!(error.kind, ErrorKind::WriteOneToClear(_)),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn snapshot_acknowledgement_checks_every_possible_clear_value() -> TestResult {
    for constraint in [
        "write-range = { min = 1, max = 1 }",
        "write-range = { min = 0, max = 0 }",
        "write-values-only = true\nvalues = [{ name = \"NONE\", value = 0 }]",
        "write-values-only = true\nvalues = [{ name = \"EVENT\", value = 1 }]",
    ] {
        let fixture = Fixture::new()?;
        fixture
            .edit(
                "types/timer.toml",
                "name = \"T0\"\nbits = \"0\"\nclass = \"write-one-to-clear\"",
                &format!(
                    "name = \"T0\"\nbits = \"0\"\nclass = \"write-one-to-clear\"\n{constraint}"
                ),
            )?
            .remove("fixed-register-images", "acknowledge_timer0_masked")?
            .remove("fixed-register-images", "acknowledge_timer0_consumed")?;
        let error = fixture.rejection()?;
        assert_eq!(error.entity, "timer0_interrupt");
        assert!(
            matches!(error.kind, ErrorKind::WriteConstraint(_)),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn snapshot_acknowledgement_checks_unsampled_zero_lanes() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"",
            "name = \"T1\"\nbits = \"2\"\nclass = \"write-one-to-clear\"\nwrite-range = { min = 1, max = 1 }",
        )?
        .remove("fixed-register-images", "acknowledge_timer0_masked")?
        .remove("fixed-register-images", "acknowledge_timer0_consumed")?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "timer0_interrupt");
    assert!(
        matches!(error.kind, ErrorKind::WriteConstraint(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn snapshot_acknowledgement_checks_sparse_clear_values() -> TestResult {
    for (constraint, admitted) in [
        ("write-range = { min = 0, max = 5 }", true),
        ("write-range = { min = 0, max = 4 }", false),
        (
            "write-values-only = true\nvalues = [{ name = \"NONE\", value = 0 }, { name = \"FIRST\", value = 1 }, { name = \"SECOND\", value = 4 }, { name = \"BOTH\", value = 5 }]",
            true,
        ),
        (
            "write-values-only = true\nvalues = [{ name = \"NONE\", value = 0 }, { name = \"FIRST\", value = 1 }, { name = \"BOTH\", value = 5 }]",
            false,
        ),
    ] {
        let fixture = Fixture::new()?;
        fixture
            .edit(
                "types/timer.toml",
                "name = \"T1\"\nbits = \"1\"\nclass = \"read-only\"",
                "name = \"T1\"\nbits = \"2\"\nclass = \"read-only\"",
            )?
            .edit(
                "types/timer.toml",
                "name = \"T0\"\nbits = \"0\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"",
                &format!("name = \"EVENTS\"\nbits = \"2:0\"\nclass = \"write-one-to-clear\"\n{constraint}"),
            )?
            .remove("fixed-register-images", "acknowledge_timer0_masked")?
            .remove("fixed-register-images", "acknowledge_timer0_consumed")?;
        if admitted {
            fixture.load()?;
        } else {
            let error = fixture.rejection()?;
            assert_eq!(error.entity, "timer0_interrupt");
            assert!(
                matches!(error.kind, ErrorKind::WriteConstraint(_)),
                "{error}"
            );
        }
    }
    Ok(())
}

#[test]
fn fixed_writes_keep_whole_word_and_field_class_exceptions() -> TestResult {
    for class in ["set-only", "self-clearing", "hardware-written"] {
        let fixture = Fixture::new()?;
        fixture
            .edit(
                "types/timer.toml",
                "name = \"LOAD\"\nbits = \"0\"\nclass = \"write-trigger\"",
                &format!("name = \"LOAD\"\nbits = \"0\"\nclass = \"{class}\""),
            )?
            .edit(
                "types/timer.toml",
                "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"",
                "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"\n\n[[registers.fields]]\nname = \"STATUS\"\nbits = \"8\"\nclass = \"read-only\"",
            )?
            .append(
                "\n[[fixed-register-images]]\nname = \"store_load\"\nregister = \"TIMER0.LOAD\"\nvalue = 1\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-images]]\nname = \"store_key_whole\"\nregister = \"TIMER0.WO\"\nvalue = 256\nwhole-word = true\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-sequences]]\nname = \"sequence_load\"\nsteps = [{ register = \"TIMER0.LOAD\", value = 0 }, { register = \"TIMER0.WO\", value = 256, whole-word = true }]\nsources = [\"SRC_DRIVER\"]\n",
            )?;
        fixture.load()?;
    }
    Ok(())
}

#[test]
fn whole_word_image_keeps_to_its_trigger_fields() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(
        "\n[[fixed-register-images]]\nname = \"escape_split_word\"\nregister = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x80000001\nwhole-word = true\nsources = [\"SRC_DRIVER\"]\n",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::ReachesUnclaimed(_)));
    Ok(())
}

#[test]
fn fixed_input_fits_its_projection() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "name = \"enable_timer0\"\nfields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 1",
        "name = \"enable_timer0\"\nfields = [{ field = \"TIMER0.CONFIG.EN\", source-bit-offset = 0 }]\nvalue = 2",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)));
    Ok(())
}

#[test]
fn domain_fits_its_projection() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit("policy.toml", "max = 0xffff", "max = 0x1ffff")?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)));
    Ok(())
}

#[test]
fn transform_retains_bits_its_projection_takes() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "retain-mask = 0xffff",
        "retain-mask = 0x1ffff",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)));
    Ok(())
}

#[test]
fn bit_modify_takes_a_closed_domain() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(
        "\n[[field-or-modifies]]\nname = \"or_alarm\"\nfields = [{ field = \"TIMER0.ALARM_%s.ALARM\", source-bit-offset = 0 }]\ndomain = \"AlarmImage\"\nsources = [\"SRC_DRIVER\"]\n",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Domain(_)));
    Ok(())
}

#[test]
fn hardware_written_field_is_stored_only_as_a_data_port() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"ALARM\"\nbits = \"31:0\"\nclass = \"plain\"",
        "name = \"ALARM\"\nbits = \"31:0\"\nclass = \"hardware-written\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::FieldClass(_)));
    fixture.edit(
        "types/timer.toml",
        "name = \"ALARM_%s\"\noffset = 0x20",
        "name = \"ALARM_%s\"\noffset = 0x20\ndata-port = { sources = [\"SRC_HEADER\"] }",
    )?;
    fixture.load()?;
    Ok(())
}

#[test]
fn modify_replays_no_hardware_written_field() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"",
            "name = \"DIV\"\nbits = \"15:0\"\nclass = \"hardware-written\"",
        )?
        .remove("zero-register-writes", "clear_timer0_config")?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "set_timer0_divider");
    assert!(matches!(error.kind, ErrorKind::Register(_)), "{error}");
    Ok(())
}

#[test]
fn modify_replays_no_self_clearing_field() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"EN\"\nbits = \"31\"\nclass = \"plain\"",
            "name = \"EN\"\nbits = \"31\"\nclass = \"self-clearing\"",
        )?
        .remove("zero-register-writes", "clear_timer0_config")?
        .remove("field-or-modifies", "enable_timer0")?
        .remove("field-clear-modifies", "disable_timer0")?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "set_timer0_divider");
    assert!(matches!(error.kind, ErrorKind::Register(_)), "{error}");
    Ok(())
}

#[test]
fn write_constraints_bound_transactions() -> TestResult {
    // An opaque value exceeds a write range.
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"ALARM\"\nbits = \"31:0\"\nclass = \"plain\"",
        "name = \"ALARM\"\nbits = \"31:0\"\nclass = \"plain\"\nwrite-range = { min = 1, max = 255 }",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::WriteConstraint(_)));
    // A write from zero gives zero to a field that takes none.
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"",
        "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"\nwrite-range = { min = 1, max = 255 }",
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::WriteConstraint(_)),
        "{error}"
    );
    // A bounded domain inside the range is a value the field takes.
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"",
            "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"\nwrite-range = { min = 0, max = 15 }",
        )?
        .remove("masked-register-modifies", "set_timer0_divider")?
        .remove("field-replace-modifies", "replace_timer0_divider_from_argument")?
        .remove("indexed-bit-set-modifies", "set_timer0_divider_bit")?
        .edit(
            "policy.toml",
            "value = 0x40000001\nwhole-word = true",
            "value = 0x40000000\nwhole-word = true",
        )?
        .edit("policy.toml", "domain = \"Divider\"", "domain = \"BitIndex\"")?;
    fixture.load()?;
    Ok(())
}

#[test]
fn full_read_returns_every_word_value() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "domain = \"ValueImage\"",
        "domain = \"Event\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Domain(_)));
    Ok(())
}
