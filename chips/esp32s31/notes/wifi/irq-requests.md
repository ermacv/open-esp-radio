# Wi-Fi interrupt-bank writes requested from task context

Two register transactions on the MAC and WDEVPWR interrupt banks apply while
an interrupt epoch is live: the connected-STA beacon-filter disable and the
WDEVPWR wake-cause mask and acknowledgement. No live path in this repository
applies them; only the cold path (before the CPU route is bound) applies the
beacon-filter disable. This note records their facts.

## Banks

| Bank | Base | Register (offset) |
| --- | --- | --- |
| `WIFI_MAC_INTERRUPT` | `0x2010_4c40` | ENABLE (`+0x0`) |
| `WIFI_MAC_STA_BEACON_FILTER` | `0x2010_42b4` | CONTROL (`+0x0`) |
| `WIFI_MAC_POWER_INTERRUPT` (WDEVPWR) | `0x2010_d8b4` | ENABLE (`+0x0`), RAW (`+0x4`), STATUS (`+0x8`, masked), CLEAR (`+0xc`, write-one-to-clear) |

WDEVPWR field geometry, identical in ENABLE, RAW, STATUS and CLEAR:
`TBTT_3` bit 0, `TBTT_2` bit 1, `TBTT_1` bit 2, `TBTT_0` bit 3 (TSF 0 is the
station TSF), `TSF_TIMER_3` bit 4, `TSF_TIMER_2` bit 5, `TSF_TIMER_1` bit 6,
`TSF_TIMER_0` bit 7, bits 31:8 opaque.

## Connected STA without power save: beacon-filter disable

SOURCE: complete `libpp[hal_mac.o]::hal_disable_sta_beacon_filter` (size
`0x22`; register evidence `BLOB_LIBPP_HAL_RUNTIME_MMIO_LEAVES`).

1. Read-modify-write `WIFI_MAC_STA_BEACON_FILTER`.CONTROL: clear bits 2:0
   (the three opaque bits change together; their individual meanings are
   unknown).
2. A second, independent fresh read-modify-write of `WIFI_MAC_INTERRUPT`.ENABLE:
   clear bit 15 (`STA_BEACON_FILTER`).

The order is fixed: the filter hardware is disabled before its interrupt
source is masked. The enable leaf `hal_enable_sta_beacon_filter` (size `0x2c`)
runs in the opposite order: it enables ENABLE bit 15 first, then sets CONTROL
bits 2:0.

Bit 15 is clear in the cold receive mask (`0x19a8_79e0`) already, so in the
cold path step 2 leaves the published mask unchanged.

## WDEVPWR wake cause: mask and acknowledge

SOURCE: the guarded TSF-timer leaves of `libpp[hal_tsf.o]` (evidence
`BLOB_LIBPP_HAL_TSF_TIMER`) and its station TBTT leaves (evidence
`BLOB_LIBPP_HAL_TSF_TBTT_20260927`) identify the CLEAR bits; complete
`libpp[hal_tsf.o]::hal_pwr_interrupt_get_event` reads the masked STATUS at
`0x2010_d8bc` and `libpp[hal_tsf.o]::hal_pwr_interrupt_clr_event` stores its
full argument to CLEAR at `0x2010_d8c0` (evidence
`BLOB_LIBPP_HAL_PWR_INTERRUPT`). The step order below is the transaction the
removed `mask_and_acknowledge_wake_cause` applied.

Mask and acknowledge one cause, in this order:

1. Write the fixed image `0x0000_0000` to WDEVPWR ENABLE: every cause is
   masked.
2. One zero-based write to WDEVPWR CLEAR with only the cause's bit set (every
   other bit written as zero):
   - TSF timer 0: bit 7 (`TSF_TIMER_0`);
   - TSF timer 1: bit 6 (`TSF_TIMER_1`);
   - TSF timer 2: bit 5 (`TSF_TIMER_2`);
   - TSF timer 3: bit 4 (`TSF_TIMER_3`);
   - station TBTT: bit 3 (`TBTT_0`).
3. Device fence.

Acknowledge without masking is step 2 then step 3, with ENABLE untouched.

Only these five causes have reviewed bit identities. Beacon-miss, modem-limit
and RF causes are among the opaque bits; their identities are unknown. The
reviewed SVD has no writer for a partial ENABLE image, so the mask is the
complete zero image, not a single cause: it is a rollback or handoff, not a
proof that the selected cause can wake production firmware.

The runtime enable set at epoch activation is ENABLE bit 3 (`TBTT_0`) alone
(complete `libpp[hal_tsf.o]::hal_enable_sta_tbtt`).

## On the radio doorbell

Each of these is a request on the radio doorbell: the radio executor (P2)
sets a request bit and rings the doorbell; the P3 line, which owns both
interrupt banks, applies the transaction in its doorbell handler. Task context
never writes the banks of a live epoch. W1's doorbell carries only the
`UNMASK_RX` and `MASK_ALL` requests.
