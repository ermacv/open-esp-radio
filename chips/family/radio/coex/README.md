# Espressif coexistence priorities

`oer-chips-family-radio-coex` holds the coexistence event priority table the
Espressif chips share, recovered from ESP32-S31 esp-coex-lib `fa152b3b`: the
49 events (`CoexEventId`), the four-bit priority domain (`CoexPti`) and the
vendor's cold table (`CoexPtiTable::VENDOR`), equal in the pinned ESP32-S31
and ESP32-C5 archives. SOURCE: pinned `libcoexist.a`
([vendor pins](../../../esp32s31/vendor/artifacts.toml)).

It owns no register and keeps no state. No code of this tree changes the
table: a chip reads the priority of an event from the constant. On the
ESP32-S31 the radio domain answers `RadioDomain::coex_pti` from it, Wi-Fi's
cold initialisation publishes those priorities to its MAC, and the PHY's
grant-protect request on timer 5 takes the priority of event 48
([`oer-chips-esp32s31-hal`](../../../esp32s31/hal/README.md)).

`CoexPti` admits only the four-bit domain, so a table entry or a value a
MAC field receives needs no runtime range check. A priority is a request to
the hardware arbiter, never a grant.
