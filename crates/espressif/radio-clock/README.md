# Shared modem clock planner

`oer-radio-clock` is the reference-count planner of ESP-IDF's
`components/esp_hw_support/modem/modem_clock.c`, which every chip shares.
Radio modules (Wi-Fi, Bluetooth, IEEE 802.15.4, PHY, coexistence, ...)
request fixed sets of modem clock dependencies; the planner counts each
dependency and emits a physical edge only when a count goes from zero to one
or from one to zero, visiting the dependencies lowest vendor device first. It
reproduces the vendor's two exceptions: a dependency whose refcount another
owner keeps emits its edge on every request, and the Wi-Fi clock
dependencies stay enabled while Wi-Fi is initialized.

A chip's HAL supplies:

- its vendor device order as a `ModemClockDependency` table
  (`table_is_consistent` checks the table's indices);
- its module dependency sets as `DependencySet`s;
- the physical action of each edge, passed to `execute_acquire` and
  `execute_release`. An edge counts as performed only when the action
  succeeds; a failed edge poisons the transaction, because the planner cannot
  tell a refused request from a partially applied one.

Preparation is transactional: counts and lease slots change only when every
edge was performed and the transaction commits. Leases are opaque, bound to
one planner epoch and checked against stale, duplicate and cross-planner
release. The crate performs no MMIO; the ESP32-S31 table and executor are in
`oer_esp32s31_hal::power::clock`.
