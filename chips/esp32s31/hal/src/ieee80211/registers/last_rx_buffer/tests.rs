//! The last-RX-buffer table initialization over host memory.

use crate::ieee80211::registers::HostWords;

#[test]
#[should_panic(expected = "PAC gap")]
fn the_whole_word_parameter_images_wait_for_their_pac_accessor() {
    // Entries zero to four store `ENTRY_PARAMETER_B%s` as whole words wider
    // than its one described field; the thin PAC has no accessor for them.
    let mut words = HostWords::filled(0);
    words.owners().registers.initialize_mac_last_rx_buffer();
}
