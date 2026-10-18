//! Recording boundary shared by the compiled vendor driver.
//!
//! The compiled C driver reaches Rust only through the `extern "C"` functions
//! in this module. Each call appends one [`Record`]; getters and register
//! reads answer from the scenario [`Inputs`] or, for a register-layer getter
//! with a matching setter, from the last value that setter wrote. The model
//! deliberately knows nothing else about the hardware.

use std::{
    collections::BTreeMap,
    ffi::{CStr, c_char, c_void},
    fmt,
    sync::Mutex,
};

/// One register-layer argument.
///
/// Buffer addresses are labelled instead of printed, so a trace does not
/// depend on where the host placed the driver's statics or the scenario's
/// frames.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Argument {
    /// A scalar value.
    Value(u64),
    /// The `n`th frame the scenario passed to `esp_ieee802154_transmit`.
    TransmitFrame(usize),
    /// The `n`th distinct driver-owned buffer, in order of first appearance.
    DriverBuffer(usize),
    /// The bytes an address, key or extended-address pointer refers to,
    /// read after the call.
    Bytes(Vec<u8>),
}

impl fmt::Display for Argument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => write!(formatter, "{value:#x}"),
            Self::TransmitFrame(index) => write!(formatter, "tx#{index}"),
            Self::DriverBuffer(index) => write!(formatter, "buf#{index}"),
            Self::Bytes(bytes) => write!(formatter, "[{}]", hex(bytes)),
        }
    }
}

/// One observation at the vendor driver boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Record {
    /// A register-layer (`ieee802154_ll_*`) accessor call.
    Ll {
        name: String,
        arguments: Vec<Argument>,
    },
    /// A call leaving the driver: PHY, BTBB, coexistence, clocks, interrupt
    /// allocation, time or the critical section.
    External { name: String, arguments: Vec<u64> },
    /// A coexistence priority the driver publishes through libcoexist
    /// (`esp_coex_ieee802154_txrx_pti_set`, `esp_coex_ieee802154_ack_pti_set`)
    /// with its level argument.
    Coex { name: String, level: u64 },
    /// A direct register access outside the LL (the ETM helpers).
    RegisterWrite { address: u32, value: u32 },
    /// A direct register read outside the LL.
    RegisterRead { address: u32, value: u32 },
    /// Return value of one public driver call made by the scenario.
    Return { name: String, value: i32 },
    /// An application callback delivered by the driver.
    Event {
        name: String,
        arguments: Vec<u64>,
        first: Vec<u8>,
        second: Vec<u8>,
    },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn list(arguments: &[u64]) -> String {
    arguments
        .iter()
        .map(|argument| format!("{argument:#x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ll { name, arguments } => write!(
                formatter,
                "ll {name}({})",
                arguments
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::External { name, arguments } => {
                write!(formatter, "ext {name}({})", list(arguments))
            }
            Self::Coex { name, level } => write!(formatter, "coex {name}({level:#x})"),
            Self::RegisterWrite { address, value } => {
                write!(formatter, "reg-write {address:#010x} = {value:#x}")
            }
            Self::RegisterRead { address, value } => {
                write!(formatter, "reg-read {address:#010x} -> {value:#x}")
            }
            Self::Return { name, value } => write!(formatter, "return {name} = {value}"),
            Self::Event {
                name,
                arguments,
                first,
                second,
            } => write!(
                formatter,
                "event {name}({}) [{}] [{}]",
                list(arguments),
                hex(first),
                hex(second)
            ),
        }
    }
}

/// Values the scenario supplies to vendor reads.
#[derive(Default)]
pub struct Inputs {
    /// Return value of a named getter or external call.
    pub values: BTreeMap<String, u64>,
    /// Return value of `esp_ieee802154_enh_ack_generator`; `None` refuses.
    pub enhanced_ack: Option<Vec<u8>>,
    /// The key of a secured enhanced ACK, which the generator arms as the
    /// OpenThread port does.
    pub enhanced_ack_key: Option<[u8; 16]>,
}

/// The register-layer value model shared by the vendor recorder and the port
/// runner: a scenario input answers a named getter, otherwise a getter
/// returns the last value its setter wrote.
#[derive(Default)]
pub(crate) struct LlModel {
    pub(crate) inputs: Inputs,
    /// Last value written by each register-layer setter, keyed by the setter
    /// name and its leading (index) arguments.
    written: BTreeMap<(String, Vec<u64>), u64>,
    /// Extended addresses copied at write time, keyed by multi-PAN index.
    extended_addresses: BTreeMap<u64, [u8; 8]>,
}

#[derive(Default)]
struct State {
    records: Vec<Record>,
    model: LlModel,
    registers: BTreeMap<u32, u32>,
    rx_address: Option<usize>,
    handler: Option<(usize, usize)>,
    transmit_frames: Vec<usize>,
    driver_buffers: Vec<usize>,
}

impl State {
    fn label(&mut self, address: usize) -> Argument {
        if let Some(index) = self
            .transmit_frames
            .iter()
            .position(|&frame| frame == address)
        {
            return Argument::TransmitFrame(index);
        }
        let index = match self
            .driver_buffers
            .iter()
            .position(|&buffer| buffer == address)
        {
            Some(index) => index,
            None => {
                self.driver_buffers.push(address);
                self.driver_buffers.len() - 1
            }
        };
        Argument::DriverBuffer(index)
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(action: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    action(guard.get_or_insert_with(State::default))
}

pub(crate) fn reset(inputs: Inputs) {
    with_state(|state| {
        *state = State {
            model: LlModel::new(inputs),
            ..State::default()
        }
    });
}

pub(crate) fn take_records() -> Vec<Record> {
    with_state(|state| core::mem::take(&mut state.records))
}

pub(crate) fn set_input(name: &str, value: u64) {
    with_state(|state| {
        state.model.inputs.values.insert(name.to_owned(), value);
    });
}

pub(crate) fn record_return(name: &str, value: i32) {
    with_state(|state| {
        state.records.push(Record::Return {
            name: name.to_owned(),
            value,
        })
    });
}

pub(crate) fn register_transmit_frame(address: usize) {
    with_state(|state| state.transmit_frames.push(address));
}

pub(crate) fn rx_address() -> Option<usize> {
    with_state(|state| state.rx_address)
}

pub(crate) fn interrupt_handler() -> Option<(usize, usize)> {
    with_state(|state| state.handler)
}

fn name_of(name: *const c_char) -> String {
    // SAFETY: every caller in the compiled stand passes a static,
    // NUL-terminated string literal.
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

fn arguments_of(arguments: *const u64, count: u32) -> Vec<u64> {
    if arguments.is_null() || count == 0 {
        return Vec::new();
    }
    // SAFETY: the generated recorder and host glue pass a stack array of
    // exactly `count` values that outlives this call.
    unsafe { std::slice::from_raw_parts(arguments, count as usize) }.to_vec()
}

fn bytes_of(bytes: *const u8, len: u32) -> Vec<u8> {
    if bytes.is_null() || len == 0 {
        return Vec::new();
    }
    // SAFETY: the host glue passes a live driver buffer of `len` bytes.
    unsafe { std::slice::from_raw_parts(bytes, len as usize) }.to_vec()
}

const LL: &str = "ieee802154_ll_";

/// Pointer arguments recorded by content: `(accessor, argument, bytes)`.
pub(crate) const BYTE_ARGUMENTS: [(&str, usize, usize); 4] = [
    ("ieee802154_ll_set_multipan_ext_addr", 1, 8),
    ("ieee802154_ll_get_multipan_ext_addr", 1, 8),
    ("ieee802154_ll_set_security_addr", 0, 8),
    ("ieee802154_ll_set_security_key", 0, 16),
];
const EXTENDED_ADDRESS_SETTER: &str = "ieee802154_ll_set_multipan_ext_addr";
const EXTENDED_ADDRESS_GETTER: &str = "ieee802154_ll_get_multipan_ext_addr";

impl LlModel {
    /// A model answering from `inputs` with nothing written yet.
    pub(crate) fn new(inputs: Inputs) -> Self {
        Self {
            inputs,
            ..Self::default()
        }
    }

    /// Answer one register-layer call.
    pub(crate) fn call(&mut self, name: &str, arguments: &[u64]) -> u64 {
        if name == EXTENDED_ADDRESS_SETTER
            && let [index, pointer] = *arguments
        {
            let mut address = [0; 8];
            // SAFETY: the driver passes its eight-byte extended-address buffer.
            unsafe { std::ptr::copy_nonoverlapping(pointer as *const u8, address.as_mut_ptr(), 8) };
            self.extended_addresses.insert(index, address);
            return 0;
        }
        if name == EXTENDED_ADDRESS_GETTER
            && let [index, pointer] = *arguments
        {
            let address = self
                .extended_addresses
                .get(&index)
                .copied()
                .unwrap_or_default();
            // SAFETY: the driver passes an eight-byte output buffer.
            unsafe { std::ptr::copy_nonoverlapping(address.as_ptr(), pointer as *mut u8, 8) };
            return 0;
        }
        if let Some(value) = self.inputs.values.get(name) {
            return *value;
        }
        if let Some(suffix) = name.strip_prefix("ieee802154_ll_set_")
            && let Some((value, index)) = arguments.split_last()
        {
            self.written
                .insert((format!("{LL}get_{suffix}"), index.to_vec()), *value);
            return 0;
        }
        self.written
            .get(&(name.to_owned(), arguments.to_vec()))
            .copied()
            .unwrap_or(0)
    }
}

/// The libcoexist calls that publish the driver's coexistence levels; the
/// port compares them, unlike the other calls leaving the driver.
pub const COEX_PRIORITY_CALLS: [&str; 2] = [
    "esp_coex_ieee802154_txrx_pti_set",
    "esp_coex_ieee802154_ack_pti_set",
];

/// The ESP32-S31 BTBB provider's transmit-power level set, which the driver
/// reads through its weak `bt_bb_get_tx_pwr_table`. The stand supplies the
/// level set recovered from the vendor library, the one the production engine
/// resolves against.
#[unsafe(no_mangle)]
extern "C" fn bt_bb_get_tx_pwr_table(length: *mut u8) -> *const i8 {
    let levels = &oer_esp32s31_hal::phy::baseband::TX_POWER_LEVELS_DBM;
    if !length.is_null() {
        // SAFETY: the driver passes a pointer to its local `uint8_t length`.
        unsafe { length.write(levels.len() as u8) };
    }
    levels.as_ptr()
}

/// Record one LL or external call and return the modelled value.
#[unsafe(no_mangle)]
extern "C" fn oer_host_record(
    name: *const c_char,
    arguments: *const u64,
    count: u32,
    pointers: u32,
) -> u64 {
    let name = name_of(name);
    let arguments = arguments_of(arguments, count);
    with_state(|state| {
        if name.starts_with(LL) {
            if name == "ieee802154_ll_set_rx_addr" {
                state.rx_address = arguments.first().map(|&address| address as usize);
            }
            let value = state.model.call(&name, &arguments);
            let arguments = arguments
                .iter()
                .enumerate()
                .map(|(index, &argument)| {
                    let by_content = BYTE_ARGUMENTS
                        .iter()
                        .find(|(accessor, position, _)| *accessor == name && *position == index);
                    if let Some((_, _, length)) = by_content {
                        // SAFETY: the driver passes a live buffer of the
                        // accessor's fixed length.
                        Argument::Bytes(
                            unsafe { std::slice::from_raw_parts(argument as *const u8, *length) }
                                .to_vec(),
                        )
                    } else if pointers & 1 << index != 0 {
                        state.label(argument as usize)
                    } else {
                        Argument::Value(argument)
                    }
                })
                .collect();
            state.records.push(Record::Ll { name, arguments });
            value
        } else if COEX_PRIORITY_CALLS.contains(&name.as_str())
            && let [level] = *arguments
        {
            state.records.push(Record::Coex { name, level });
            0
        } else {
            let value = state.model.inputs.values.get(&name).copied().unwrap_or(0);
            state.records.push(Record::External { name, arguments });
            value
        }
    })
}

/// Record one application callback with up to two frame buffers.
#[unsafe(no_mangle)]
extern "C" fn oer_host_event(
    name: *const c_char,
    arguments: *const u64,
    count: u32,
    first: *const u8,
    first_len: u32,
    second: *const u8,
    second_len: u32,
) {
    let record = Record::Event {
        name: name_of(name),
        arguments: arguments_of(arguments, count),
        first: bytes_of(first, first_len),
        second: bytes_of(second, second_len),
    };
    with_state(|state| state.records.push(record));
}

/// Answer the application enhanced-ACK generator from the scenario.
#[unsafe(no_mangle)]
extern "C" fn oer_host_enh_ack(
    frame: *const u8,
    frame_len: u32,
    enhack_frame: *mut u8,
    key: *mut u8,
) -> i32 {
    let frame = bytes_of(frame, frame_len);
    with_state(|state| {
        state.records.push(Record::Event {
            name: "enh_ack_generator".to_owned(),
            arguments: Vec::new(),
            first: frame,
            second: state.model.inputs.enhanced_ack.clone().unwrap_or_default(),
        });
        match &state.model.inputs.enhanced_ack {
            Some(ack) => {
                // SAFETY: the driver passes its 128-byte enhanced-ACK buffer,
                // and scenarios supply at most 128 bytes.
                unsafe { std::ptr::copy_nonoverlapping(ack.as_ptr(), enhack_frame, ack.len()) };
                match state.model.inputs.enhanced_ack_key {
                    Some(secured) => {
                        // SAFETY: the shim passes a sixteen-byte key buffer.
                        unsafe { std::ptr::copy_nonoverlapping(secured.as_ptr(), key, 16) };
                        1
                    }
                    None => 0,
                }
            }
            None => -1,
        }
    })
}

/// Capture the handler registered by `esp_intr_alloc`.
#[unsafe(no_mangle)]
extern "C" fn oer_host_interrupt_handler(handler: *const c_void, argument: *mut c_void) {
    with_state(|state| {
        state.handler = (!handler.is_null()).then_some((handler as usize, argument as usize));
    });
}

#[unsafe(no_mangle)]
extern "C" fn oer_host_register_read(address: u32) -> u32 {
    with_state(|state| {
        let value = state.registers.get(&address).copied().unwrap_or(0);
        state.records.push(Record::RegisterRead { address, value });
        value
    })
}

#[unsafe(no_mangle)]
extern "C" fn oer_host_register_write(address: u32, value: u32) {
    with_state(|state| {
        state.registers.insert(address, value);
        state.records.push(Record::RegisterWrite { address, value });
    });
}
