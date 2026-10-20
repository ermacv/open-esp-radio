//! CPU-owned byte operations on the generated RX packet prefix.

#![forbid(unsafe_code)]
use oer_esp32s31_hal::layouts::*;
use vcell::VolatileCell;

/// SOURCE: pinned `memMgmt_3.o` receive allocator and receive-recycling bodies.
const REARMED_RESULT: BleRxPacketPrefixResult = BleRxPacketPrefixResult::from_bits(0)
    .with_bit_0_opaque(true)
    .with_bit_1_opaque(true)
    .with_bit_2_opaque(true)
    .with_bit_3_opaque(true)
    .with_bit_4_opaque(true)
    .with_bits_5_8_opaque(0xf)
    .expect("sentinel fits")
    .with_bit_9_opaque(true)
    .with_bit_10_opaque(true)
    .with_bits_11_23_opaque(0x1fff)
    .expect("sentinel fits");

pub(crate) const fn result_not_produced(result: BleRxPacketPrefixResult) -> bool {
    result.bit_0_opaque()
        && result.bit_1_opaque()
        && result.bit_2_opaque()
        && result.bit_3_opaque()
        && result.bit_4_opaque()
        && result.bits_5_8_opaque() == REARMED_RESULT.bits_5_8_opaque()
        && result.bit_9_opaque()
        && result.bit_10_opaque()
        && result.bits_11_23_opaque() == REARMED_RESULT.bits_11_23_opaque()
}

pub(crate) const fn result_rejected(result: BleRxPacketPrefixResult) -> bool {
    result.bit_0_opaque()
        || result.bit_1_opaque()
        || result.bit_2_opaque()
        || result.bit_3_opaque()
        || result.bit_4_opaque()
        || result.bits_5_8_opaque() != 0
        || result.bit_9_opaque()
        || result.bit_10_opaque()
        || result.bits_11_23_opaque() != 0
}

/// Bytes after the completion gate, including the generated prefix and bounded payload.
#[repr(C, align(4))]
pub(crate) struct RxPacketBytes {
    prefix: [VolatileCell<u8>; BleRxPacketPrefix::SIZE],
    pub(crate) payload: [VolatileCell<u8>; u8::MAX as usize],
}

impl RxPacketBytes {
    pub(crate) const fn zeroed() -> Self {
        Self {
            prefix: [const { VolatileCell::new(0) }; BleRxPacketPrefix::SIZE],
            payload: [const { VolatileCell::new(0) }; u8::MAX as usize],
        }
    }

    pub(crate) const fn armed() -> Self {
        // SOURCE: pinned DTM RX allocation's capacity and producer/epoch sentinels.
        const WORDS: [[u8; 4]; 7] = [
            [0; 4],
            BleRxPacketPrefixByte0004Opaque::from_bits(0)
                .with_bits_0_31_opaque(0x0001_0100)
                .bits()
                .to_le_bytes(),
            [0; 4],
            REARMED_RESULT.bits().to_le_bytes(),
            [0; 4],
            [0; 4],
            BleRxPacketPrefixAuxiliary::from_bits(0)
                .with_bits_0_15_opaque(0xffff)
                .bits()
                .to_le_bytes(),
        ];
        let mut storage = Self::zeroed();
        let mut word = 0;
        while word < WORDS.len() {
            let mut byte = 0;
            while byte < WORDS[word].len() {
                storage.prefix[word * WORDS[word].len() + byte] =
                    VolatileCell::new(WORDS[word][byte]);
                byte += 1;
            }
            word += 1;
        }
        storage
    }

    pub(crate) fn prefix(&self) -> [u8; BleRxPacketPrefix::SIZE] {
        core::array::from_fn(|index| self.prefix[index].get())
    }

    /// Caller holds CPU ownership of the packet: before publication or after completion/fence.
    pub(crate) fn update_prefix(&self, update: impl FnOnce(&mut BleRxPacketPrefixMut<'_>)) {
        let mut bytes = self.prefix();
        update(&mut BleRxPacketPrefixMut::new(&mut bytes));
        for (cell, value) in self.prefix.iter().zip(bytes) {
            cell.set(value);
        }
    }

    pub(crate) fn clear(&self) {
        for byte in self.prefix.iter().chain(&self.payload) {
            byte.set(0);
        }
    }
    pub(crate) fn initialize(&self) {
        self.clear();
        self.update_prefix(|prefix| {
            prefix.set_byte_0004_opaque(
                BleRxPacketPrefixByte0004Opaque::from_bits(0).with_bits_0_31_opaque(0x0001_0100),
            )
        });
        self.rearm();
    }
    pub(crate) fn rearm(&self) {
        self.update_prefix(|prefix| {
            prefix.set_result(REARMED_RESULT.with_rssi(prefix.result().rssi()));
            prefix.set_auxiliary(prefix.auxiliary().with_bits_0_15_opaque(u16::MAX));
        });
    }
    #[cfg(test)]
    pub(crate) fn model_prefix(&self, bytes: impl IntoIterator<Item = u8>) {
        for (cell, byte) in self.prefix.iter().zip(bytes) {
            cell.set(byte);
        }
    }
}
