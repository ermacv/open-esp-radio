/* The Wi-Fi datapath's code a turn runs, in SRAM: `.hot.text`.
 *
 * A turn reaches more code than the 32 KiB L1 instruction cache holds, and
 * ordinary code lies in PSRAM behind it: from there a turn's time follows its
 * cache misses, and the code's place moves a hart's load by ten points
 * between builds. Protocol crates forbid unsafe code and so the
 * `link_section` attribute, and a generic function has its section only once
 * monomorphized: this list selects by name instead. Each pattern matches the
 * v0 mangled name, `<length><identifier>` of crate, module and function; it
 * names no hash, and an image without Wi-Fi matches none of it.
 *
 * Together under 100 KiB, with the SRAM it takes checked by the linker. A
 * test in this crate holds each name to a function in the sources. Rare work
 * (association, teardown, settings) stays in PSRAM.
 */
*(.text.*31oer_protocols_ieee80211_runtime8datapath*5Actor*4poll*)
*(.text.*25oer_chips_family_executor4task9poll_task*8datapath*)
*(.text.*31oer_protocols_ieee80211_runtime8datapath4turn*15collect_sources*)
*(.text.*31oer_protocols_ieee80211_runtime8datapath4turn*6staged*)
*(.text.*31oer_protocols_ieee80211_runtime8datapath4data*6settle*)
*(.text.*31oer_protocols_ieee80211_runtime8datapath4data*6intake*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver4port6driver*13service_arena*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver4port6driver*5group*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver4port6driver*7collect*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver4port6driver*7publish*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver2rx7session*12port_service*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver4port7engines*6intake*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver4port7engines*9with_port*7publish*)
*(.text.*35oer_chips_esp32s31_ieee80211_driver9lower_mac*14submit_planned*)
*(.text.*38oer_protocols_ieee80211_core_roles_sta4data*14publication_of*)
