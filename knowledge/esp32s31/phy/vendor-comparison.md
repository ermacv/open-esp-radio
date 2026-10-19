# PHY: facts from the vendor comparison

Source: typed vendor comparison scenarios of a stack outside this repository,
state of 2026-10-17. Each scenario executed the pinned vendor archive and ROM code
(`knowledge/esp32s31/vendor/artifacts.toml`) and a compiled probe of the
production PHY/HAL entry under declared peripheral inputs, and compared every
MMIO, fence and requested-delay event. The scenarios and the binary-analysis
tool were not carried; the facts below are what they established about the
vendor code. They are software relations under modelled peripherals, never
timing or RF results.

Common to every comparison: analog I2C transport reads (busy prechecks of the
two command ports), read-mask and host-map writes, and the 1 µs wait before a
transport or status read are plumbing that production does differently;
everything else matched in order and value.

## Analog I2C

- **Command-memory initializer.** The archive/ROM initializer issues 45
  ordered command writes; its descriptor and no-op leaves produce the same
  descriptor bytes from zero-filled and filled memory.
- **Transport.** Thirteen host selectors, byte and masked read/write on both
  hosts with immediate and delayed completion, and reset from idle and busy
  were compared. Reset aborts the pending command; it restores neither bank
  values nor samples. Production checks readiness before each issue; the
  vendor does not.
- **Out-of-field write.** A value wider than its field spills into the
  adjacent bit in the vendor code; the typed production path clips it. This is
  a deliberate difference.

## Calibration leaves

- TX-gain restore requires the explicitly enabled domain.
- **Temperature to power** (current archive policy): a positive Wi-Fi delta is
  divided by eight, a negative one by three; Bluetooth uses five and four. A
  similar-looking ROM function has a different policy and must not be used.
- Post-init AGC executes the ROM saturation-gain child. The archive places AGC
  in one `.iram1` section together with unrelated functions; its relocations
  need the SDK clock symbol beside the ROM symbols.

## PBus and DCODE prefix

- **PBus.** On a stuck command the production timeout leaves the command
  unacknowledged and does not restore work mode afterwards.
- **DCODE.** Four crystal selectors, eight readiness samples and forty CKGEN
  commands; four frequency programs. With the scenario's sample profile the
  eight output bytes are `[0, 63, 31, 32, 32, 31, 63, 0]`. A stuck channel
  readiness, a stuck I2C command, or a read-modify-write whose halves together
  exceed the production budget returns its own failure, leaves all eight output
  bytes unchanged and issues no further command.

## RFPLL search and frequency maintenance

- **Search** settles 5 µs after each step; signed underflow and the
  511-capacitor boundary are part of the matrix.
- **Maintenance** with a nonzero correction reads and rewrites all 85
  frequency-memory entries, preserving unrelated bits, and restores the
  selected channel; it requests 2 µs, then the search settles. Channels 1, 13
  and 14 and the frequencies 2412 and 2484 MHz were covered. On a stuck
  command production returns its timeout with the first command pending and
  does not restore hardware frequency control.
- For nonzero maintenance only, the vendor samples the channel status once more
  just before it reads frequency control (a layout query); production owns that
  fixed layout and omits the read.
- `phy_printf` diagnostics are reachable from the linked archive section; with
  diagnostics enabled the vendor path calls into the SDK image.

## Gain arithmetic and publication

- The gain object has a 216-byte coefficient section; the callback copies from
  it in three `memcpy` ranges.
- 18 Wi-Fi and 18 Bluetooth coefficient thresholds select the intervals. A
  Wi-Fi calculation produces 160 output bytes, a Bluetooth one 80.
- Complete publication is 161 MMIO events for Wi-Fi and 81 for Bluetooth,
  including bank selection, index wrap and preserved control bits.
- The current archive applies an **additive** Wi-Fi adjustment; the older ROM
  callback uses **subtractive** attenuation with its own tables. The two
  profiles are not equivalent.
- Bluetooth publication goes through the callback installed by
  `phy_get_romfunc_addr`; without the installer nothing is published.

## Gain state and RF-test power

Vendor-only: no production storage or RF power API exists for these.

- The captured image's startup adjustment byte is zero.
- The calibration backup image is 532 bytes; backup, loss of live state,
  recovery, parameter registration and consumption of the restored adjustment
  were executed for adjustments 0, 1, 31, 127, 128 and 255.
- `librftest.a` `set_rate_power_index` and `mac_power_set`: rounding,
  saturation, signed-byte wrap and the adjustment byte decide the index;
  `phy_wifi_set_tx_gain_new` is called conditionally; both MAC power-index
  words are written. A missing callback installation publishes neither gain
  nor MAC state.

## Low-power clocks (ESP-IDF `libesp_hw_support.a`, `libhal.a`)

- **Bluetooth.** `modem_clock_select_lp_clock_source(PERIPH_BT_MODULE,
  MAIN_XTAL, 399)` and `modem_clock_deselect_lp_clock_source(PERIPH_BT_MODULE)`
  match the production lease selection and deselection register for register;
  the driver's critical section and sleep power-domain bookkeeping have no
  register effect.
- **Wi-Fi.** `esp_perip_clk_init` calls
  `modem_clock_select_lp_clock_source(PERIPH_WIFI_MODULE, source, 0)`; the
  register effects match production for the RC slow oscillator and the 32 kHz
  crystal. Production selects once at the first cold radio power-up and never
  deselects; the vendor keeps a record of the source and a Wi-Fi power-clock
  reference count.
