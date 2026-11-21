//! The coexistence runtime leaves over host memory.

use crate::radio::ieee80211::registers::{HostWords, MacItwtArgument, MacPti};

/// `hal_set_itwt_pti` stores the `seqz` of its argument in the ITWT
/// control byte: one for a zero argument, zero for any other.
#[test]
fn the_itwt_control_byte_is_one_for_a_zero_argument_and_zero_otherwise() {
    for (argument, byte) in [(MacItwtArgument::Zero, 1), (MacItwtArgument::Nonzero, 0)] {
        for fill in [0, u32::MAX] {
            let mut memory = HostWords::filled(fill);
            let shared = MacPti::new(5).expect("a PTI");
            memory.owners().registers.set_itwt_pti(argument, shared);
            let owners = memory.owners();
            let runtime = &owners.registers.mac.wifi_mac_coex_runtime;
            assert_eq!(runtime.read_itwt_control().bits_0_7_opaque(), byte);
            assert_eq!(runtime.read_shared_rx_pti().pti().get(), 5);
        }
    }
}
