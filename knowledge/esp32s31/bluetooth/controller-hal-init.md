# ESP32-S31 Bluetooth Controller HAL initialization

Source: `esp32s31-bt-lib` `libbtdm_common.a` (pin `10c50778`), member `7.o`
`r_sym_bt_aGdrujd2MUAzWYH75baR` with its configuration setter and byte-scaling
helper, and the caller `r_btdm_task_init` in member `20.o`; extracted from
the predecessor repository's `crates/esp32s31/pac/src/bluetooth/controller/init.rs`
and its `init/tests.rs`.

Facts only. Fixed values are the ones in the reviewed register model,
`registers/esp32s31/policy/api.toml` (operations named
`*_bluetooth_hal_*`), and are not repeated here.

## Place in the lifecycle

The body runs on the Controller enable path just before the Controller's IRQ
output is prepared. It does not set up software events or lists, route a CPU
interrupt, enable the Link Layer or make HCI ready. Clocks must already be
enabled.

## Inputs and accepted domains

- Scale: the byte 8 or 16. No time unit is established.
- Two bytes, `value_0` and `value_1`, with unknown hardware meaning.
- Period: 500, 1000 or 2000. No time unit is established.

The pinned Controller task passes scale 16, bytes 11 and 33, and period 2000.

## Order of writes

All registers are in the Controller core block at `0x2010_1000`. The body
makes 49 writes or read-modify-writes, in this order:

1. `0x2010_1074` `SCHEDULER_SRAM_POINTER_PREFIX`: fixed image, the SRAM
   prefix of compressed scheduler pointers.
2. `0x2010_1090` `BTLC_TIMER_CONTROL` bits 2:0 `CLOCK_SHIFT`: the sleep-timer
   shift (see below). Bits 31:24 are left as they are.
3. `0x2010_1084` `HAL_INIT_BYTES` bits 7:0 `VALUE_0`: `value_0` after the
   period-dependent byte scaling.
4. `0x2010_1084` bits 15:8 `VALUE_1`: `value_1` after the same scaling.
5. `0x2010_11c0` `HAL_INIT_LATCH`: complete image with only bit 6 set.
6. `0x2010_11c4` `HAL_INIT_LOW_20`: complete image with bits 19:0 all set.
7. `0x2010_11c0` bit 31: set.
8. `0x2010_10e0` `WDT_CONFIG_1`: bits 15, 19 and 18 set.
9. `0x2010_10e0`: bits 3, 7 and 6 set.
10. `0x2010_10dc` `WDT_CONFIG_0` bit 31 `ENABLE`: set.
11. `0x2010_1250` `HAL_INIT_SCHEDULER_CONTROL` bits 20:16: clear.
12. `0x2010_1250` bits 20:16: fixed value.
13. `0x2010_1250` bit 31: set.
14. `0x2010_127c` `HAL_INIT_LOW_HALF` bits 15:0: clear.
15. `0x2010_127c` bits 15:0: all set.
16. `0x2010_1250` bits 15:8: clear.
17. `0x2010_1250` bits 15:8: fixed value.
18. to 49. Slot map: sixteen four-bit lanes in `HAL_INIT_SLOT_MAP0`
    (`0x2010_134c`, lanes 0 to 7) and `HAL_INIT_SLOT_MAP1` (`0x2010_1350`,
    lanes 8 to 15), two RMWs per lane in lane order. The resulting lane
    layout is described in the register model: bits 3:2 cleared, then bit 0
    set, bit 1 set (never cleared) for odd lane index, and bit 2 equal to bit
    1 of the lane index. The meaning of the words is unknown.

Steps 8 and 9 and step 10 are watchdog configuration words of
`r_obh_link_wdt_config`; the other names are positional.

## Controller tick rate

With period 2000 (the pinned profile) the raw Controller time counts two
ticks per microsecond. The vendor converts with `r_btdm_sleep_timer_ticks_get`,
`r_btdm_hal_util_ticks_to_us` and `r_sched_timer_convertTimeToUs`, and back
with `r_sched_timer_convertTimeToTicks`. The period also selects this software
conversion; periods 500 and 1000 give other rates and are not used. The
rounding policy for odd tick counts and for the inverse is ours to design.
Counter width, wrap and wake causality are not established.

## To be measured or designed

- `sleep_timer_shift`: a table maps each of the six (scale, period) pairs to
  the three-bit `CLOCK_SHIFT` of step 2. Its values are to be measured on
  hardware (read `0x2010_1090` bits 2:0 after vendor initialization; only the
  pinned pair can be observed that way) and otherwise designed by us.
- Byte scaling: the period selects a scaling of `value_0` and `value_1`
  before steps 3 and 4, keeping only the low byte. The scaling is to be
  measured (read `0x2010_1084` after vendor initialization) and our helper
  arithmetic is to be designed independently.
- Slot map: confirm the lane layout by reading `0x2010_134c` and
  `0x2010_1350` after vendor initialization.
