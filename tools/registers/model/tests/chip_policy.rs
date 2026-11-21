//! The ownership policy over a chip root on synthetic fixtures: one policy
//! that uses every construct, and one rejection per load-time rule.

use std::{error::Error as StdError, fs};

use oer_tools_registers_model::{
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
pointer = { points-to = "RX_DESCRIPTOR", placement = "dma-window", encoding = { width = 32, shift = 0 }, sources = ["SRC_HEADER"] }

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

[[interrupt-pairs]]
status = "INT_RAW"
clear = "INT_CLR"
sources = ["SRC_HEADER"]
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
interrupt-sources = { count = 2, sources = ["SRC_HEADER"] }
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

[[interrupts]]
name = "TIMER0_INTR"
source = 0
instance = "TIMER0"
sources = ["SRC_HEADER"]

[[interrupts]]
name = "TIMER1_INTR"
source = 1
instance = "TIMER1"
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
schema = 7
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
interrupt = "TIMER0_INTR"
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

[[interrupt-snapshots]]
name = "timer0_interrupt"
status = "TIMER0.INT_RAW"
sources = ["SRC_DRIVER"]

[[partition-image-reads]]
name = "radio_image"
partition = "RadioPeripherals"
sources = ["SRC_DRIVER"]

[[zero-based-field-writes]]
name = "write_timer0_key"
fields = ["TIMER0.WO.KEY"]
whole-word-bits = { value = 0xff000000, sources = ["SRC_HEADER"] }
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
whole-word = true
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

[[field-or-modifies]]
name = "point_timer0_descriptor"
fields = [{ field = "TIMER0.ADDR.ADDR", source-bit-offset = 0 }]
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
    assert_eq!(owner("write_timer0_key"), Some(part(0, 0)));
    assert_eq!(owner("point_timer0_descriptor"), Some(part(0, 0)));
    assert_eq!(owner("enable_channel0"), Some(part(0, 2)));
    assert_eq!(owner("route_channel2"), Some(part(1, 0)));
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
    [("schema = 7", "schema = 6")],
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

// Transactions: names and references.
rejects!(
    transaction_name,
    [(
        "name = \"point_timer0_descriptor\"",
        "name = \"PointTimer0Descriptor\""
    )],
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_transaction,
    [(
        "name = \"enable_channel0\"",
        "name = \"store_timer0_config_word\""
    )],
    ErrorKind::Duplicate(_)
);
rejects!(
    transaction_sources,
    [(
        "source-bit-offset = 0 }]\nsources = [\"SRC_DRIVER\"]",
        "source-bit-offset = 0 }]\nsources = []"
    )],
    ErrorKind::InvalidSources
);
rejects!(
    unknown_field,
    [(
        "fields = [\"TIMER0.WO.KEY\"]",
        "fields = [\"TIMER0.WO.LOCK\"]"
    )],
    ErrorKind::UnknownField(_)
);
rejects!(
    unknown_transaction_register,
    [(
        "register = \"TIMER0.CONFIG\"\nvalue",
        "register = \"TIMER0.CONFIGURATION\"\nvalue"
    )],
    ErrorKind::UnknownRegister(_)
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
    pointer_without_hardware_fact,
    [(
        "field = \"TIMER0.ADDR.ADDR\"",
        "field = \"TIMER0.CONFIG.DIV\""
    )],
    ErrorKind::Pointer(_)
);
rejects!(
    projections_of_one_register,
    [(
        "fields = [{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }]",
        "fields = [{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }, { field = \"TIMER0.ALARM_0.ALARM\", source-bit-offset = 0 }]"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    projection_input_bits,
    [(
        "{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }",
        "{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 1 }"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    pointer_or_takes_one_pointer_field,
    [(
        "{ field = \"TIMER0.ADDR.ADDR\", source-bit-offset = 0 }",
        "{ field = \"TIMER0.CONFIG.DIV\", source-bit-offset = 0 }"
    )],
    ErrorKind::Pointer(_)
);
// Transactions: field classes.
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
    store_of_fifo,
    [(
        "fields = [\"TIMER0.WO.KEY\"]",
        "fields = [\"TIMER0.FIFO.DATA\"]"
    )],
    ErrorKind::Register(_)
);
rejects!(
    zero_register_write_of_a_word_that_is_not_read_observed,
    [(
        "[[fixed-register-images]]\nname = \"acknowledge_timer0_masked\"",
        "[[zero-register-writes]]\nname = \"clear_timer0_config\"\nregister = \"TIMER0.CONFIG\"\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-images]]\nname = \"acknowledge_timer0_masked\""
    )],
    ErrorKind::Register(_)
);
rejects!(
    handler_part_names_its_interrupt,
    [("interrupt = \"TIMER0_INTR\"\n", "")],
    ErrorKind::Interrupt(_)
);
rejects!(
    part_interrupt_is_bound_to_its_instance,
    [("interrupt = \"TIMER0_INTR\"", "interrupt = \"TIMER1_INTR\"")],
    ErrorKind::Interrupt(_)
);
rejects!(
    one_part_serves_an_interrupt,
    [(
        "name = \"Task\"\ninstance = \"TIMER0\"\n",
        "name = \"Task\"\ninstance = \"TIMER0\"\ninterrupt = \"TIMER0_INTR\"\n"
    )],
    ErrorKind::Interrupt(_)
);
/// A second source of `TIMER0` beside `TIMER0_INTR`, as a part whose one
/// bank acknowledges two sources needs.
fn two_timer_sources() -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "root.toml",
            "interrupt-sources = { count = 2,",
            "interrupt-sources = { count = 3,",
        )?
        .edit(
            "root.toml",
            "[[interrupts]]\nname = \"TIMER1_INTR\"",
            "[[interrupts]]\nname = \"TIMER0_NRT_INTR\"\nsource = 2\ninstance = \"TIMER0\"\nsources = [\"SRC_HEADER\"]\n\n[[interrupts]]\nname = \"TIMER1_INTR\"",
        )?;
    Ok(fixture)
}

#[test]
fn part_serves_every_source_its_bank_acknowledges() -> TestResult {
    let fixture = two_timer_sources()?;
    fixture.edit(
        "policy.toml",
        "interrupt = \"TIMER0_INTR\"",
        "interrupt = [\"TIMER0_INTR\", \"TIMER0_NRT_INTR\"]",
    )?;
    let policy = fixture.load()?;
    let irq = policy
        .file()
        .partitions
        .iter()
        .flat_map(|partition| &partition.parts)
        .find(|part| part.name == "Irq")
        .ok_or("the fixture has its Irq part")?;
    assert_eq!(irq.interrupt, ["TIMER0_INTR", "TIMER0_NRT_INTR"]);
    Ok(())
}

#[test]
fn every_listed_interrupt_is_bound_to_the_part_instance() -> TestResult {
    let fixture = two_timer_sources()?;
    fixture.edit(
        "policy.toml",
        "interrupt = \"TIMER0_INTR\"",
        "interrupt = [\"TIMER0_INTR\", \"TIMER1_INTR\"]",
    )?;
    assert!(matches!(fixture.rejection()?.kind, ErrorKind::Interrupt(_)));
    Ok(())
}

#[test]
fn a_listed_interrupt_is_served_once() -> TestResult {
    let fixture = two_timer_sources()?;
    fixture
        .edit(
            "policy.toml",
            "interrupt = \"TIMER0_INTR\"",
            "interrupt = [\"TIMER0_INTR\", \"TIMER0_NRT_INTR\"]",
        )?
        .edit(
            "policy.toml",
            "name = \"Task\"\ninstance = \"TIMER0\"\n",
            "name = \"Task\"\ninstance = \"TIMER0\"\ninterrupt = \"TIMER0_NRT_INTR\"\n",
        )?;
    assert!(matches!(fixture.rejection()?.kind, ErrorKind::Interrupt(_)));
    Ok(())
}

rejects!(
    a_list_names_each_interrupt_once,
    [(
        "interrupt = \"TIMER0_INTR\"",
        "interrupt = [\"TIMER0_INTR\", \"TIMER0_INTR\"]"
    )],
    ErrorKind::Interrupt(_)
);
rejects!(
    handler_part_names_an_interrupt_in_its_list,
    [("interrupt = \"TIMER0_INTR\"", "interrupt = []")],
    ErrorKind::Interrupt(_)
);
rejects!(
    part_interrupt_is_a_source_of_the_root,
    [("interrupt = \"TIMER0_INTR\"", "interrupt = \"MISSING\"")],
    ErrorKind::Interrupt(_)
);
rejects!(
    snapshot_status_needs_an_interrupt_pair,
    [("status = \"TIMER0.INT_RAW\"", "status = \"TIMER0.VALUE\"")],
    ErrorKind::Snapshot(_)
);
rejects!(
    snapshot_names_elements,
    [(
        "status = \"TIMER0.INT_RAW\"",
        "status = \"TIMER0.ALARM_%s\""
    )],
    ErrorKind::Snapshot(_)
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
        "register = \"TIMER0.CONFIG\"\nvalue",
        "register = \"TIMER1.CONFIG\"\nvalue"
    )],
    ErrorKind::ReachesUnclaimed(_)
);
#[test]
fn transaction_on_unclaimed_template() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(&element_range_sequence(
        "TIMER1.ALARM_%s",
        "first = 0, count = 2, ",
    ))?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::ReachesUnclaimed(_)));
    Ok(())
}
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
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000001",
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000000"
    )],
    ErrorKind::Transaction(_)
);
rejects!(
    transaction_on_shared_part,
    [(
        "register = \"ETM.CHANNEL_ENABLE_SET\"\nvalue = 0x00000001",
        "register = \"COUNTER.UPDATE\"\nvalue = 0x00000001"
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
            "register = \"TIMER0.CONFIG\"\nvalue = 0x40000001",
            "register = \"TIMER1.INT_ENA\"\nvalue = 0x00000001"
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

const FORGE_IMAGE: &str = "\n[[fixed-register-images]]\nname = \"forge_pointer\"\nregister = \"TIMER0.ADDR\"\nvalue = 0xdeadbeef\nsources = [\"SRC_DRIVER\"]\n";

#[test]
fn integer_writers_cannot_forge_a_pointer() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(FORGE_IMAGE)?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn hardware_pointer_is_protected_without_a_policy_pointer_operation() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .remove("field-or-modifies", "point_timer0_descriptor")?
        .append(FORGE_IMAGE)?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Pointer(_)));
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
            "\n[[zero-based-field-writes]]\nname = \"zero_clear\"\nfields = [\"TIMER0.INT_CLR.T0\"]\nwhole-word-bits = { value = 0x80000000, sources = [\"SRC_HEADER\"] }\nsources = [\"SRC_DRIVER\"]\n",
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
        // The type's interrupt pair already refuses an unreadable status.
        let error = match fixture.load() {
            Ok(_) => return Err("the fixture loaded".into()),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("interrupt pair"), "{error}");
    }
    Ok(())
}

const INT_CLR_W1C: &str = "name = \"INT_CLR\"\noffset = 0x0c\n\n[[registers.fields]]\nname = \"T0\"\nbits = \"0\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"";

/// The fixture with `INT_CLR` as the vendor's `INT_CLR`: write-trigger
/// strobes, `t1` the bits of its second field.
fn write_trigger_clear(t1: &str) -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        INT_CLR_W1C,
        &format!("name = \"INT_CLR\"\noffset = 0x0c\n\n[[registers.fields]]\nname = \"T0\"\nbits = \"0\"\nclass = \"write-trigger\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"{t1}\"\nclass = \"write-trigger\""),
    )?;
    Ok(fixture)
}

#[test]
fn snapshot_acknowledges_through_write_trigger_strobes() -> TestResult {
    let policy = write_trigger_clear("1")?.load()?;
    assert_eq!(
        policy.owner("timer0_interrupt").map(Holder::Part),
        Some(part(0, 1))
    );
    assert_eq!(
        policy.owner("acknowledge_timer0_masked").map(Holder::Part),
        Some(part(0, 1))
    );
    Ok(())
}

#[test]
fn write_trigger_clear_field_acknowledges_only_sampled_bits() -> TestResult {
    let error = write_trigger_clear("2:1")?.rejection()?;
    assert_eq!(error.entity, "timer0_interrupt");
    assert!(matches!(error.kind, ErrorKind::Snapshot(_)), "{error}");
    Ok(())
}

#[test]
fn write_trigger_clear_word_is_written_only_by_acknowledgements() -> TestResult {
    let fixture = write_trigger_clear("1")?;
    fixture.append(
        "\n[[zero-based-field-writes]]\nname = \"zero_clear\"\nfields = [\"TIMER0.INT_CLR.T0\"]\nwhole-word-bits = { value = 0x80000000, sources = [\"SRC_HEADER\"] }\nsources = [\"SRC_DRIVER\"]\n",
    )?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "zero_clear");
    assert!(
        matches!(error.kind, ErrorKind::WriteOneToClear(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn write_one_to_clear_word_is_its_own_status() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "policy.toml",
        "status = \"TIMER0.INT_RAW\"",
        "status = \"TIMER0.INT_CLR\"",
    )?;
    assert_eq!(
        fixture.load()?.owner("timer0_interrupt").map(Holder::Part),
        Some(part(0, 1))
    );
    // A word of other fields is no status of its own.
    let fixture = write_trigger_clear("1")?;
    fixture.edit(
        "policy.toml",
        "status = \"TIMER0.INT_RAW\"",
        "status = \"TIMER0.INT_CLR\"",
    )?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "timer0_interrupt");
    assert!(matches!(error.kind, ErrorKind::Snapshot(_)), "{error}");
    Ok(())
}

#[test]
fn every_fixed_write_needs_a_writable_word() -> TestResult {
    for (value, whole_word) in [(0u32, false), (0, true), (0xdeadbeef, true)] {
        for sequence in [false, true] {
            let fixture = Fixture::new()?;
            let operation = if sequence {
                format!(
                    "[[fixed-register-sequences]]\nname = \"write_counter\"\nsteps = [{{ register = \"TIMER0.VALUE\", value = {value} }}]"
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
fn w1c_status_word_accepts_a_fixed_discard_only_with_its_consumption_proof() -> TestResult {
    for (proof, accepted) in [
        (
            Some("The startup epoch discards all inherited pending."),
            true,
        ),
        (None, false),
        (Some(" "), false),
    ] {
        let fixture = Fixture::new()?;
        fixture
            .remove("interrupt-snapshots", "timer0_interrupt")?
            .remove("fixed-register-images", "acknowledge_timer0_masked")?
            .remove("fixed-register-images", "acknowledge_timer0_consumed")?;
        let proof = proof.map_or(String::new(), |text| format!("consumed-event = {text:?}\n"));
        fixture.append(&format!("\n[[fixed-register-images]]\nname = \"discard_startup\"\nregister = \"TIMER0.INT_CLR\"\nvalue = 3\n{proof}sources = [\"SRC_DRIVER\"]\n"))?;
        if accepted {
            fixture.load()?;
        } else {
            assert_eq!(fixture.rejection()?.entity, "discard_startup");
        }
    }
    Ok(())
}

/// A fixed sequence of one template step over `register` with `range`
/// (TOML key-values) and `whole_word`.
fn element_range_sequence(register: &str, range: &str) -> String {
    format!(
        "\n[[fixed-register-sequences]]\nname = \"store_alarms\"\nsteps = [{{ register = \"{register}\", {range}value = 0 }}]\nsources = [\"SRC_DRIVER\"]\n"
    )
}

#[test]
fn an_element_range_writes_the_elements_it_selects() -> TestResult {
    for range in ["first = 0, count = 2, ", "first = 1, count = 1, "] {
        let fixture = Fixture::new()?;
        fixture.append(&element_range_sequence("TIMER0.ALARM_%s", range))?;
        fixture.load()?;
    }
    Ok(())
}

#[test]
fn an_element_range_outside_its_array_is_refused() -> TestResult {
    for range in [
        "first = 0, count = 3, ",
        "first = 2, count = 1, ",
        "first = 1, count = 0, ",
        "first = 4294967295, count = 2, ",
    ] {
        let fixture = Fixture::new()?;
        fixture.append(&element_range_sequence("TIMER0.ALARM_%s", range))?;
        let error = fixture.rejection()?;
        assert_eq!(error.entity, "store_alarms");
        assert!(
            matches!(error.kind, ErrorKind::ElementRange(_)),
            "{range}: {error}"
        );
    }
    // A range names a template, not one element.
    let fixture = Fixture::new()?;
    fixture.append(&element_range_sequence(
        "TIMER0.ALARM_0",
        "first = 0, count = 1, ",
    ))?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::ElementRange(_)), "{error}");
    Ok(())
}

#[test]
fn an_element_range_is_a_complete_whole_word_sequence() -> TestResult {
    // `first` and `count` go together, on a template or one element alike.
    for register in ["TIMER0.ALARM_%s", "TIMER0.ALARM_0"] {
        for range in ["first = 0, ", "count = 1, "] {
            let fixture = Fixture::new()?;
            fixture.append(&element_range_sequence(register, range))?;
            let error = fixture.rejection()?;
            assert_eq!(error.entity, "store_alarms");
            assert!(
                matches!(error.kind, ErrorKind::Transaction(_)),
                "{register} {range}: {error}"
            );
        }
    }
    // It is its sequence's only step.
    let fixture = Fixture::new()?;
    fixture.append("\n[[fixed-register-sequences]]\nname = \"store_alarms\"\nsteps = [{ register = \"TIMER0.ALARM_%s\", first = 0, count = 2, value = 0 }, { register = \"TIMER0.CONFIG\", value = 0 }]\nsources = [\"SRC_DRIVER\"]\n")?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "store_alarms");
    assert!(matches!(error.kind, ErrorKind::Transaction(_)), "{error}");
    Ok(())
}

#[test]
fn every_element_of_a_range_takes_the_step_rules() -> TestResult {
    // Elements two parts hold are no one owner's.
    let fixture = Fixture::new()?;
    fixture.append(&element_range_sequence(
        "ETM.CH%s_EVENT",
        "first = 1, count = 2, ",
    ))?;
    assert_eq!(fixture.rejection()?.entity, "store_alarms");
    // An element with no writable field takes no fixed write.
    let fixture = Fixture::new()?;
    fixture.write(
        "types/timer.toml",
        &TIMER.replace(
            "name = \"ALARM\"\nbits = \"31:0\"\nclass = \"plain\"",
            "name = \"ALARM\"\nbits = \"31:0\"\nclass = \"read-only\"",
        ),
    )?;
    fixture.append(&element_range_sequence(
        "TIMER0.ALARM_%s",
        "first = 0, count = 2, ",
    ))?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "store_alarms");
    assert!(matches!(error.kind, ErrorKind::Register(_)), "{error}");
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
            "name = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"T2\"\nbits = \"2\"\nclass = \"write-one-to-clear\"\nwrite-range = { min = 1, max = 1 }",
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
                "\n[[fixed-register-images]]\nname = \"store_load\"\nregister = \"TIMER0.LOAD\"\nvalue = 1\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-images]]\nname = \"store_key_whole\"\nregister = \"TIMER0.WO\"\nvalue = 256\nwhole-word = true\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-sequences]]\nname = \"sequence_load\"\nsteps = [{ register = \"TIMER0.LOAD\", value = 0 }, { register = \"TIMER0.WO\", value = 256 }]\nsources = [\"SRC_DRIVER\"]\n",
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
fn a_write_from_zero_takes_no_hardware_written_field_and_no_data_port() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"",
        "name = \"KEY\"\nbits = \"7:0\"\nclass = \"hardware-written\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::FieldClass(_)));
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"WO\"\noffset = 0x30",
        "name = \"WO\"\noffset = 0x30\ndata-port = { sources = [\"SRC_HEADER\"] }",
    )?;
    fixture.edit(
        "types/timer.toml",
        "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"",
        "name = \"KEY\"\nbits = \"7:0\"\nclass = \"plain\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Register(_)));
    Ok(())
}

/// The pointer OR reads, modifies and writes back its word: a word whose
/// write-back writes an unreadable field or a port is refused, and so is a
/// field the modify cannot replace.
#[test]
fn a_pointer_or_modifies_only_a_word_it_may_write_back() -> TestResult {
    for (class, fifo) in [("write-only", false), ("plain", true)] {
        let fixture = Fixture::new()?;
        fixture.edit(
            "types/timer.toml",
            "name = \"ADDR\"\nbits = \"31:0\"\nclass = \"plain\"",
            &format!("name = \"ADDR\"\nbits = \"31:0\"\nclass = \"{class}\""),
        )?;
        if fifo {
            fixture.edit(
                "types/timer.toml",
                "name = \"ADDR\"\noffset = 0x28",
                "name = \"ADDR\"\noffset = 0x28\nfifo = { sources = [\"SRC_HEADER\"] }",
            )?;
        }
        let error = fixture.rejection()?;
        assert_eq!(error.entity, "point_timer0_descriptor", "{class}: {error}");
        assert!(
            matches!(error.kind, ErrorKind::Register(_)),
            "{class}: {error}"
        );
    }
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timer.toml",
        "name = \"ADDR\"\nbits = \"31:0\"\nclass = \"plain\"",
        "name = \"ADDR\"\nbits = \"31:0\"\nclass = \"read-only\"",
    )?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "point_timer0_descriptor", "{error}");
    assert!(matches!(error.kind, ErrorKind::FieldClass(_)), "{error}");
    Ok(())
}

#[test]
fn write_constraints_bound_transactions() -> TestResult {
    let ranged = |fixture: &Fixture| -> TestResult {
        fixture.edit(
            "types/timer.toml",
            "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"",
            "name = \"DIV\"\nbits = \"15:0\"\nclass = \"plain\"\nwrite-range = { min = 1, max = 255 }",
        )?;
        Ok(())
    };
    // The image gives the field a value its range admits.
    let fixture = Fixture::new()?;
    ranged(&fixture)?;
    fixture.load()?;
    // An image gives zero to a field that takes none.
    let fixture = Fixture::new()?;
    ranged(&fixture)?;
    fixture.edit(
        "policy.toml",
        "value = 0x40000001\nwhole-word = true",
        "value = 0x40000000\nwhole-word = true",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::WriteConstraint(_)));
    Ok(())
}

/// The fixture with a `T2` acknowledgement in `INT_CLR` that no status
/// field samples, with `constraint`, and the timer snapshot consuming
/// `fields` for `event`.
fn consumed_snapshot(constraint: &str, event: &str, fields: &str) -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            INT_CLR_W1C,
            &format!("{INT_CLR_W1C}\n\n[[registers.fields]]\nname = \"T2\"\nbits = \"2\"\nclass = \"write-one-to-clear\"\n{constraint}"),
        )?
        .edit(
            "policy.toml",
            "status = \"TIMER0.INT_RAW\"\n",
            &format!("status = \"TIMER0.INT_RAW\"\n{event}{fields}"),
        )?;
    Ok(fixture)
}

const CONSUMED_EVENT: &str = "consumed-event = \"The caller read timer 2's event elsewhere.\"\n";
const CONSUMED_T2: &str = "consumed-fields = [\"TIMER0.INT_CLR.T2\"]\n";

#[test]
fn snapshot_acknowledges_a_consumed_event_beside_its_sample() -> TestResult {
    let policy = consumed_snapshot("", CONSUMED_EVENT, CONSUMED_T2)?.load()?;
    assert_eq!(
        policy.owner("timer0_interrupt").map(Holder::Part),
        Some(part(0, 1))
    );
    let fields = &policy.file().interrupt_snapshots[0].consumed_fields;
    assert_eq!(fields, &["TIMER0.INT_CLR.T2"]);
    Ok(())
}

#[test]
fn consumed_event_lane_takes_its_constraint() -> TestResult {
    // Unconsumed, the lane is written zero, which the field admits.
    consumed_snapshot("write-range = { min = 0, max = 0 }", "", "")?.load()?;
    let error = consumed_snapshot(
        "write-range = { min = 0, max = 0 }",
        CONSUMED_EVENT,
        CONSUMED_T2,
    )?
    .rejection()?;
    assert_eq!(error.entity, "timer0_interrupt");
    assert!(
        matches!(error.kind, ErrorKind::WriteConstraint(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn consumed_event_names_one_bit_acknowledgements_of_its_clear_word() -> TestResult {
    for (event, fields, expected) in [
        ("", CONSUMED_T2, "snapshot"),
        (CONSUMED_EVENT, "", "snapshot"),
        ("consumed-event = \" \"\n", CONSUMED_T2, "description"),
        (
            CONSUMED_EVENT,
            "consumed-fields = [\"TIMER0.INT_ENA.T0\"]\n",
            "snapshot",
        ),
        (
            CONSUMED_EVENT,
            "consumed-fields = [\"TIMER0.CONFIG.DIV\"]\n",
            "snapshot",
        ),
        (
            CONSUMED_EVENT,
            "consumed-fields = [\"TIMER0.INT_CLR.T2\", \"TIMER0.INT_CLR.T2\"]\n",
            "duplicate",
        ),
    ] {
        let error = consumed_snapshot("", event, fields)?.rejection()?;
        assert_eq!(error.entity, "timer0_interrupt", "{error}");
        let matched = match expected {
            "snapshot" => matches!(error.kind, ErrorKind::Snapshot(_)),
            "description" => matches!(error.kind, ErrorKind::MissingDescription),
            _ => matches!(error.kind, ErrorKind::Duplicate(_)),
        };
        assert!(matched, "{error}");
    }
    Ok(())
}

/// The fixture with a read-only `STATUS` bit 8 beside `WO.KEY` and one
/// more write from zero, `extra`, appended.
fn whole_word_bits(extra: &str) -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"",
            "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"\n\n[[registers.fields]]\nname = \"STATUS\"\nbits = \"8\"\nclass = \"read-only\"",
        )?
        .append(extra)?;
    Ok(fixture)
}

const KEY_WHOLE: &str = "\n[[zero-based-field-writes]]\nname = \"store_key_whole\"\nfields = [\"TIMER0.WO.KEY\"]\nwhole-word-bits = { value = VALUE, sources = SOURCES }\nsources = [\"SRC_DRIVER\"]\n";

fn key_whole(value: &str, sources: &str) -> String {
    KEY_WHOLE
        .replace("VALUE", value)
        .replace("SOURCES", sources)
}

#[test]
fn write_from_zero_stores_sourced_whole_word_bits() -> TestResult {
    // A read-only bit and a bit of no field.
    let policy = whole_word_bits(&key_whole("0x80000100", "[\"SRC_HEADER\"]"))?.load()?;
    assert_eq!(
        policy.owner("store_key_whole").map(Holder::Part),
        Some(part(0, 0))
    );
    Ok(())
}

#[test]
fn whole_word_bits_reach_no_written_field() -> TestResult {
    for (extra, entity) in [
        // A plain bit outside the input fields.
        (
            "\n[[zero-based-field-writes]]\nname = \"store_enable_whole\"\nfields = [\"TIMER0.CONFIG.EN\"]\nwhole-word-bits = { value = 1, sources = [\"SRC_HEADER\"] }\nsources = [\"SRC_DRIVER\"]\n".to_owned(),
            "store_enable_whole",
        ),
        // A bit of the input fields.
        (key_whole("0x00000180", "[\"SRC_HEADER\"]"), "store_key_whole"),
    ] {
        let error = whole_word_bits(&extra)?.rejection()?;
        assert_eq!(error.entity, entity, "{error}");
        assert!(
            matches!(
                error.kind,
                ErrorKind::FieldClass(_) | ErrorKind::Transaction(_)
            ),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn whole_word_bits_cite_their_sources() -> TestResult {
    let error = whole_word_bits(&key_whole("0x00000100", "[]"))?.rejection()?;
    assert_eq!(error.entity, "store_key_whole");
    assert!(matches!(error.kind, ErrorKind::InvalidSources), "{error}");
    Ok(())
}

#[test]
fn whole_word_bits_belong_to_writes_from_zero_alone() -> TestResult {
    for (from, to) in [
        // A read-modify-write.
        (
            "name = \"point_timer0_descriptor\"\n",
            "name = \"point_timer0_descriptor\"\nwhole-word-bits = { value = 0x40000000, sources = [\"SRC_HEADER\"] }\n",
        ),
        // A snapshot.
        (
            "status = \"TIMER0.INT_RAW\"\n",
            "status = \"TIMER0.INT_RAW\"\nwhole-word-bits = { value = 4, sources = [\"SRC_HEADER\"] }\n",
        ),
    ] {
        let error = rejected(&[(from, to)])?;
        assert!(matches!(error.kind, ErrorKind::Parse(_)), "{error}");
    }
    Ok(())
}

/// The fixture with a `VALID` flag of `class` at bit 29 of the shared
/// counter's update word.
fn shared_update_flag(class: &str) -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/counter.toml",
        "name = \"UPDATE\"\nbits = \"0\"\nclass = \"write-trigger\"",
        &format!("name = \"UPDATE\"\nbits = \"0\"\nclass = \"write-trigger\"\n\n[[registers.fields]]\nname = \"VALID\"\nbits = \"29\"\nclass = \"{class}\""),
    )?;
    Ok(fixture)
}

#[test]
fn shared_strobe_leaves_a_write_one_to_clear_flag_zero() -> TestResult {
    let policy = shared_update_flag("write-one-to-clear")?.load()?;
    let Some(Claim::Whole(Holder::Part(counter))) = policy.claim("COUNTER.UPDATE") else {
        return Err("a part holds the update word".into());
    };
    assert!(policy.part(*counter).ok_or("a part")?.shared);
    Ok(())
}

#[test]
fn shared_word_never_writes_one_to_a_write_one_to_clear_flag() -> TestResult {
    // No strobe: the only write is an acknowledgement.
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/counter.toml",
        "name = \"UPDATE\"\nbits = \"0\"\nclass = \"write-trigger\"",
        "name = \"UPDATE\"\nbits = \"0\"\nclass = \"write-one-to-clear\"",
    )?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Shared(_)));
    // A strobe image that writes the flag one.
    let fixture = shared_update_flag("write-one-to-clear")?;
    fixture.append(
        "\n[[fixed-register-images]]\nname = \"strobe_and_clear\"\nregister = \"COUNTER.UPDATE\"\nvalue = 0x20000001\nsources = [\"SRC_DRIVER\"]\n",
    )?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "strobe_and_clear");
    assert!(
        matches!(
            error.kind,
            ErrorKind::WriteOneToClear(_) | ErrorKind::Register(_) | ErrorKind::Shared(_)
        ),
        "{error}"
    );
    Ok(())
}

fn record_word(register: &str) -> String {
    format!(
        "\n[[record-words]]\nregister = \"{register}\"\nreason = \"A test record keeps it.\"\nsources = [\"SRC_DRIVER\"]\n"
    )
}

#[test]
fn a_record_word_accepts_split_fields_and_a_register_array() -> TestResult {
    for register in ["TIMER0.CONFIG", "TIMER0.ALARM_%s"] {
        let fixture = Fixture::new()?;
        fixture.append(&record_word(register))?;
        fixture.load()?;
    }
    Ok(())
}

#[test]
fn a_record_word_rejects_side_effects_and_unreadable_words() -> TestResult {
    for register in ["TIMER0.FIFO", "TIMER0.LOAD", "TIMER0.WO", "TIMER0.STATE"] {
        let fixture = Fixture::new()?;
        fixture.append(&record_word(register))?;
        assert!(
            matches!(kind_of(&fixture)?, ErrorKind::Register(_)),
            "{register}"
        );
    }
    let fixture = Fixture::new()?;
    fixture
        .edit(
            "types/timer.toml",
            "name = \"CONFIG\"\noffset = 0x00",
            "name = \"CONFIG\"\noffset = 0x00\nread-side-effect = { sources = [\"SRC_HEADER\"] }",
        )?
        .append(&record_word("TIMER0.CONFIG"))?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Register(_)));
    Ok(())
}

#[test]
fn a_record_word_rejects_an_unclaimed_word() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(&record_word("TIMER2.VALUE"))?;
    let error = kind_of(&fixture)?;
    assert!(matches!(error, ErrorKind::ReachesUnclaimed(_)), "{error}");
    Ok(())
}

#[test]
fn a_record_word_requires_evidence_a_reason_and_one_entry() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(&record_word("TIMER0.CONFIG").replace("SRC_DRIVER", "MISSING"))?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::UnknownSource(_)));
    let fixture = Fixture::new()?;
    fixture.append(&record_word("TIMER0.CONFIG").replace("A test record keeps it.", " "))?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Transaction(_)));
    let fixture = Fixture::new()?;
    fixture
        .append(&record_word("TIMER0.CONFIG"))?
        .append(&record_word("TIMER0.CONFIG"))?;
    assert!(matches!(kind_of(&fixture)?, ErrorKind::Duplicate(_)));
    Ok(())
}

#[test]
fn a_record_word_rejects_a_write_one_to_clear_word() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.append(&record_word("TIMER0.INT_CLR"))?;
    let error = fixture.rejection()?;
    assert_eq!(error.entity, "TIMER0.INT_CLR");
    assert!(
        matches!(error.kind, ErrorKind::WriteOneToClear(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_record_word_rejects_each_nonobservational_lane_in_a_mixed_word() -> TestResult {
    for class in [
        "write-trigger",
        "write-one-to-clear",
        "write-only",
        "unknown",
    ] {
        let fixture = whole_word_bits("")?;
        fixture
            .remove("zero-based-field-writes", "write_timer0_key")?
            .edit(
                "types/timer.toml",
                "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"",
                &format!("name = \"KEY\"\nbits = \"7:0\"\nclass = \"{class}\""),
            )?;
        // The word and its ordinary read-only neighbour are valid before
        // naming it a record word.
        fixture.load()?;
        fixture.append(&record_word("TIMER0.WO"))?;
        let error = fixture.rejection()?;
        assert_eq!(error.entity, "TIMER0.WO");
        assert!(
            matches!(
                error.kind,
                ErrorKind::FieldClass(_) | ErrorKind::WriteOneToClear(_)
            ),
            "{class}: {error}"
        );
    }
    Ok(())
}

#[test]
fn a_record_word_accepts_each_defined_observational_class() -> TestResult {
    for class in [
        "plain",
        "read-only",
        "read-observed",
        "set-only",
        "self-clearing",
        "hardware-written",
        "debug-only",
    ] {
        let fixture = whole_word_bits("")?;
        fixture
            .remove("zero-based-field-writes", "write_timer0_key")?
            .edit(
                "types/timer.toml",
                "name = \"KEY\"\nbits = \"7:0\"\nclass = \"write-only\"",
                &format!("name = \"KEY\"\nbits = \"7:0\"\nclass = \"{class}\""),
            )?
            .append(&record_word("TIMER0.WO"))?;
        fixture.load()?;
    }
    Ok(())
}

/// Schema 7 has no table of the kinds that rendered nothing: each is an
/// unknown key, refused by its name.
#[test]
fn a_removed_kind_is_refused_by_name() -> TestResult {
    for kind in [
        "field-replace-modifies",
        "field-clear-modifies",
        "full-register-writes",
        "full-register-reads",
        "masked-register-modifies",
        "pointer-fields",
        "indexed-bit-set-modifies",
        "field-reads",
        "sampled-bit-zero-writes",
        "flag-domains",
        "enum-domains",
        "field-domains",
        "bounded-domains",
        "opaque-domains",
        "bitwise-composed-domains",
    ] {
        let fixture = Fixture::new()?;
        fixture.append(&format!("\n[[{kind}]]\nname = \"inert\"\n"))?;
        let error = fixture.rejection()?;
        assert!(
            matches!(&error.kind, ErrorKind::Parse(message) if message.contains(kind)),
            "{kind}: {error}"
        );
    }
    Ok(())
}

/// A write from zero exists for the fixed ones beside its fields, and a
/// sequence step is a whole-word image: neither can be written otherwise.
#[test]
fn a_write_from_zero_names_its_whole_word_bits_and_a_step_is_whole() -> TestResult {
    let error = rejected(&[(
        "whole-word-bits = { value = 0xff000000, sources = [\"SRC_HEADER\"] }\n",
        "",
    )])?;
    assert!(matches!(error.kind, ErrorKind::Parse(_)), "{error}");
    let error = rejected(&[(
        "{ register = \"ETM.CH2_EVENT\", value = 8 }",
        "{ register = \"ETM.CH2_EVENT\", value = 8, whole-word = true }",
    )])?;
    assert!(matches!(error.kind, ErrorKind::Parse(_)), "{error}");
    Ok(())
}
