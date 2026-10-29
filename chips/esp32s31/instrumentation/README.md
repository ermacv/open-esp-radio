# ESP32-S31 instrumentation vocabulary

`oer-esp32s31-instrumentation` contains host-safe radio observation values in `health`: counters, signals, tracking and measurements. It contains no HAL, driver, product facade, HIL transport or storage owner. `oer-esp32s31-instrumentation-hil` provides the HIL page/lifecycle binding and capture store beside this vocabulary.

`wifi` defines host-safe small driver event values for publication, terminals, EDCA and group rollback. Image collectors count kinds; this dictionary owns no counters, ring, snapshots or HAL dependency.
