# ESP32-S31 IEEE 802.15.4 lifecycle and register contracts

This reference describes public ESP-IDF source contracts pinned at commit
`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`. Source order, software state and
hardware readiness are distinct claims. Reviewed register semantics belong to
[the register model](../registers/README.md); production
implementation retains its own authority.

## Source ledger

The files were downloaded from the pinned commit and hashed locally before
review. Links include the line ranges used for the findings.

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154.c#L35-L48) | public enable/disable order | `a83716d9944d4ffba1998cc64ebb635a605b60fc77c74ae6070e83a1c617f1bc` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L897-L977) | module enable, MAC init/deinit | `9aaccfa2832cb89bfdfd98086a984269e621400a272b02926c4e088d16222830` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L1193-L1207) | guarded PHY client acquire/release | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_pib.c#L17-L39) | TX-power table dependency | `4bc94779b0c29fdfc77dcdf0c6d3d66fad5d02324aa951d9f19877bc62532cf4` |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_pib.c#L56-L127) | PIB defaults and deferred hardware update | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_util.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_util.c#L14-L60) | channel conversion and default coexistence scenes | `e1a012d5f359e2445128977e82a304ba94c100c2994729e062e26586596df38a` |
| [`components/ieee802154/driver/esp_ieee802154_util.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_util.c#L88-L92) | public weak TX-power-table fallback | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_debug.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_debug.c#L318-L324) | software statistic clear | `e4dd37b1ffc462c78a12cca7d57e4e8bd4e0e8984d542012b12ce964ee9a1812` |
| [`components/ieee802154/private_include/esp_ieee802154_util.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_util.h#L21-L35) | valid channel range | `4ca86544b16248e1d66b85cc84df8ac37bde50727189b3b242513aab22e19017` |
| [`components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c#L7-L12) | S31 module and IRQ selection | `56246b6b482752e0d217e2391acb4869ae02cd18068c5ee3b361a4b7ae110995` |
| [`components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h#L7-L12) | S31 LL includes the common LL | `a66a3562ff6ef62ffa6dda90bc08e97f0d4509bcf3817cdc57795fd97daeaa30` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L46-L122) | event and abort values | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L168-L222) | ED mode, event RMW, abort-enable RMW | same file/hash as above |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L265-L277) | frequency-code and power writes | same file/hash as above |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L362-L364) | ED-average field write | same file/hash as above |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L476-L480) | non-coexistence PTI values | same file/hash as above |
| [`components/soc/esp32s31/register/soc/ieee802154_reg.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/ieee802154_reg.h#L184-L194) | event masks; no access-class annotation | `fd3f944ac97634605083031f96c0f942af26a81a9e9a3123281c59e5719f9d9c` |
| [`components/soc/esp32s31/register/soc/ieee802154_struct.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/ieee802154_struct.h#L143-L151) | event-status bitfield; no access-class annotation | `da13c2bc78cd6ef35a4e54ddddf11ce48fda967746193f1a0ad03578a5881752` |
| [`components/esp_hw_support/modem/modem_clock.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/modem_clock.c#L57-L149) | low-bit-first loop, refcounts, ICG-before-dependencies, reset dispatch | `f6cd061416a1356a7ca3cadb84ad9d8c137971cd9c167a74507ecd57d80f7edd` |
| [`components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c#L15-L55) | S31 dependency sets | `c89ae6110cb4ca4e1208db3ef7263c729c84c84b2b5edaed3e9a3ce7c3c1e4df` |
| [`components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c#L76-L225) | concrete dependency actions | same file/hash as above |
| [`components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c#L299-L400) | device dispatch/refcounts and ICG defaults | same file/hash as above |
| [`components/esp_hw_support/modem/include/modem/modem_clock_impl.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/include/modem/modem_clock_impl.h#L23-L83) | device bit numbering | `497fbee5c4e07adb01cfa5980ee58dbf9ecbffde6b60855f8d5df82fc9027bc7` |
| [`components/esp_hw_support/modem/include/modem/modem_clock_impl.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/include/modem/modem_clock_impl.h#L104-L158) | refcount flag and ICG codes | same file/hash as above |
| [`components/esp_hw_support/include/esp_private/esp_modem_clock.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/include/esp_private/esp_modem_clock.h#L25-L69) | public shared-clock ownership contract | `3be56e98b6c110e655186e83a5c55e72879ef4ee35e668fc172aedd4334952ff` |
| [`components/hal/esp32s31/modem_clock_hal.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/modem_clock_hal.c#L23-L129) | ICG writes and PLL-source clock image | `53c1f8c25b86cbb8eff5d06633615dde605763310fa97bcd3261e9bd84cce56e` |
| [`components/hal/include/hal/modem_clock_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/include/hal/modem_clock_types.h#L15-L35) | ICG domain order | `e00978e9a623b411467d821256414e783095bedd226e6ef32b00904fcfe21075` |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L83-L145) | ETM, ZB APB/MAC, modem-security APB fields | `565e2246dfd4af00c073b8668ee0b81b2eef9992b24030d814055159fec30b0c` |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L337-L348) | ZBMAC reset pulses | same file/hash as above |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L475-L477) | Wi-Fi-BB 80x1 field | same file/hash as above |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L598-L630) | BT APB/BB fields | same file/hash as above |
| [`components/hal/esp32s31/include/hal/modem_lpcon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_lpcon_ll.h#L173-L183) | coexistence clock field | `5e6ce588f2e029aa3b895b57b0cfdad25efeacfc3cea835cdc320bcd06d5608f` |
| [`components/esp_hal_pmu/include/hal/pmu_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_pmu/include/hal/pmu_types.h#L28-L37) | ICG sleep/modem/active bit positions | `a33d59247fb46ede4a53bcbf1402498d343f8cffda2216c0bdb55a39531df5ed` |
| [`components/soc/esp32s31/include/soc/soc_caps.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/soc_caps.h#L137-L140) | Wi-Fi/BT/802.15.4 feature gates | `002387dadb652c01af2fd606d90131a67eef24dbe74234a5ae6b0bb0d32ab1e4` |
| [`components/soc/esp32s31/include/soc/soc_caps.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/soc_caps.h#L512-L566) | independent modem clocks and ICG feature gates | same file/hash as above |
| [`components/esp_hw_support/periph_ctrl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/periph_ctrl.c#L170-L244) | temporary PHY-calibration clock acquisition | `0489dd751eeb7698d8b4567a4a5b619cf8bb4a3c9e8cc84f517d5211d85d2561` |
| [`components/esp_phy/src/phy_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_init.c#L332-L475) | PHY client-set lifecycle | `1e230e72f91c4b11f35b6b623dc45cf8628961593e45774e2bf610dce9896fbf` |
| [`components/esp_phy/src/phy_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_init.c#L997-L1023) | opaque PHY registration/calibration call | same file/hash as above |
| [`components/esp_phy/src/phy_common.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_common.c#L36-L164) | PHY client-mask operations and PLL-tracking scheduler | `c602f4f6364f8fe726766bf0151e326194c82bdfe8c449b99a103225cdf89c80` |
| [`components/esp_phy/CMakeLists.txt`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/CMakeLists.txt#L15-L18) | unconditional `phy_common.c` inclusion when PHY is enabled | `ff79172451691f9cc589a323c78e35727ef2b3bf657e9ca998deb9ce15803ac1` |
| [`components/esp_phy/Kconfig`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/Kconfig#L181-L220) | PLL-tracking period and debug-only disable gate | `9e0bc9cafe445778c19e065e9b64be1689f81992dca4391d690a9fa633e4c632` |
| [`components/esp_phy/include/esp_phy_init.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_phy_init.h#L34-L42) | PHY client flag values | `700bf7b3865705b6d858e63a55fd5a880ebf298f700927cd642bee7536e4dc58` |
| [`components/esp_phy/include/esp_private/phy.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_private/phy.h#L38-L182) | PHY declarations, including opaque RF wake/close effects | `04027a4f11a0cd6c6a76478d681f7e29e4ba8ecf038d120d991bcab01735a53f` |
| [`components/esp_phy/src/btbb_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/btbb_init.c#L98-L139) | BTBB first-user initialization/refcount | `bde0cddaa033d2f34a4eaf0f1994b2d417850dba9d2fc5e6e9ee0ceb7caca3c3` |
| [`components/esp_phy/include/esp_private/btbb.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_private/btbb.h#L14-L20) | opaque BTBB declaration | `659a94ca15d9e7d5531f34c755476523d285bc2cbc0a378b58505e67facea289` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L1134-L1250) | MAC sleep-retention module and RF gating | same file/hash as `esp_ieee802154_dev.c` above |
| [`components/ieee802154/private_include/esp_ieee802154_util.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_util.h#L37-L43) | RF gating build condition | `4ca86544b16248e1d66b85cc84df8ac37bde50727189b3b242513aab22e19017` |
| [`components/soc/esp32s31/include/soc/soc_caps.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/soc_caps.h#L563-L586) | modem power-down, PAU links and REGDMA-triggered PHY retention | same file/hash as `soc_caps.h` above |
| [`components/esp_coex/include/esp_coex_i154.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_coex/include/esp_coex_i154.h#L14-L41) | coexistence priorities and opaque setters | `4989f8d5a99300cf75419d099d735e38fb27be3f083bb79e7abe8a5af0d34f3e` |

## 1. Enable, clock, and reset order

The public enable path is strictly:

1. `ieee802154_enable()` acquires `PERIPH_IEEE802154_MODULE` clocks.
2. `ieee802154_rf_enable()` acquires PHY client bit
   `PHY_MODEM_IEEE802154 = 4` when the local RF guard is closed.
3. `esp_btbb_enable()` initializes BTBB on its first user, then increments its
   independent refcount.
4. `ieee802154_mac_init()` resets and configures the MAC.

The S31 clock dependency set is exactly
`802154_MAC | BT_I154_COMMON_BB | ETM | COEXIST | WIFI_BB_80X1 |
SOC_PLL_SOURCE_CG | BT_APB`.

Before dependency acquisition, `modem_clock_module_enable()` walks all ICG
domains and ORs the defaults into the current maps. `ACTIVE` is bit 2 (`4`) and
`MODEM` is bit 1 (`2`), producing these source-confirmed default additions:

| Domain | Added map |
| --- | ---: |
| MODEM APB | `6` |
| MODEM peripheral | `4` |
| Wi-Fi | `6` |
| BT | `4` |
| modem FE | `6` |
| IEEE 802.15.4 | `4` |
| LP APB | `6` |
| analog I2C master | `6` |
| coexistence | `6` |
| Wi-Fi power | `6` |

The IEEE 802.15.4 domain setter writes both the BT and ZB maps. The operation is
an OR with the pre-existing image, so unrelated map bits must be preserved.

Dependency bits are then visited from least to most significant. With the S31
feature gates, a fresh acquisition (all relevant refcounts transition `0 -> 1`)
has this physical order:

1. Enable the 160 MHz PLL source, then write
   `HP_SYS_CLKRST.MODEM_CONF = 0x3d`.
2. Set `MODEM_LPCON.CLK_CONF.CLK_COEX_EN = 1`.
3. Set `MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_80X1_EN = 1`.
4. Set `MODEM_SYSCON.CLK_CONF.CLK_ETM_EN = 1`.
5. Set `MODEM_SYSCON.CLK_CONF1.CLK_BT_APB_EN = 1`, then
   `MODEM_SYSCON.CLK_CONF.CLK_MODEM_SEC_APB_EN = 1`.
6. Set `MODEM_SYSCON.CLK_CONF1.CLK_BTBB_EN = 1`.
7. Set `MODEM_SYSCON.CLK_CONF.CLK_ZB_APB_EN = 1`, then
   `MODEM_SYSCON.CLK_CONF.CLK_ZBMAC_EN = 1`.

This is shared ownership, not a boolean clock switch. Each dependency normally
has a refcount; an enable only performs MMIO on `0 -> 1`, and a disable only
performs MMIO on `1 -> 0`. Disable visits the same low-bit-first order rather
than reversing it. An open driver must therefore acquire/release shared clock
leases and must not clear these fields unconditionally.

The open HAL runs IEEE 802.15.4 as a client of the shared radio arbiter. Its
clock transition asks the arbiter's modem clock planner for the IEEE 802.15.4
module, which keeps one refcount per dependency and obtains the upstream
`PLL_F160M` source and the analog-I2C master clock from the platform clock
provider. Releasing the module disables only dependencies no other client
holds.

At the beginning of MAC initialization, reset is performed under the modem
clock lock in this exact order:

1. `RST_ZBMAC`: write `1`, then `0`.
2. `RST_ZBMAC_APB`: write `1`, then `0`.

There is no delay or polling between either pulse in the reviewed source.

## 2. MAC foundation values and order

After the two reset pulses, the public source performs these operations in
order:

1. Initialize the software PIB. Defaults include channel 11, auto-ACK RX/TX
   enabled, enhanced-ACK TX enabled, promiscuous mode enabled, coordinator mode
   disabled, and RX-when-idle disabled. This only marks the PIB pending; it does
   not yet program `CHANNEL` or `TXPOWER`.
2. Clear the software TX/RX statistics.
3. OR `EVENT_EN` with `IEEE802154_EVENT_MASK = 0x3fff`. In non-test builds,
   clear timer-0 bit 8, giving an exact 14-bit field image of `0x3eff`; test
   builds retain `0x3fff`.
4. OR `TX_ABORT_EVENT_EN` with `0x01868000`, representing reasons 16, 18, 19,
   24, and 25.
5. OR `RX_ABORT_EVENT_EN` with `0x00028000`, representing reasons 16 and 18.
6. Assign `ED_CFG.ED_SAMPLE_MODE = IEEE802154_ED_SAMPLE_AVG = 1`.
7. Configure coexistence conditionally:
   - with software/external coexistence, call the coexistence ACK setter with
     `MIDDLE = 2` and the TX/RX setter for the idle scene (`IDLE = 4` by the
     public default configuration);
   - otherwise assign `PTI = 3` and `HW_ACK_PTI = 3` directly.
8. Call closed `ieee802154_txon_delay_set()` of libbtbb. Its recovered
   effect is four preserving MAC field writes, in order `TXON_DELAY = 45`,
   `RXON_DELAY = 50`, `TXRX_SWITCH_DELAY = 117` and `TXOFF_DELAY = 5`, which
   the open MAC foundation writes and reads back. It does not touch the
   shared baseband, whose auxiliary transmit-on delay keeps the BTBB
   initialization value.
9. Clear the software RX-buffer queue and set the software state to idle.
10. Allocate the IRQ and initialize sleep/retention support.

A build with external coexistence (`CONFIG_ESP_COEX_EXTERNAL_COEXIST_ENABLE`)
then calls `esp_coex_ieee802154_force_rx_enable(true)`, and `ieee802154_mac_deinit`
calls it with `false` before freeing the IRQ; its operations also enter the
external-coexistence TX and RX stages. On the pinned ESP32-S31 libcoexist
(`esp32s31/libcoexist.a` sha256
`13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147`; members
`coexist_api.o` sha256
`2fef142ce7fcbd837f67989d48e8b2a9b8acb67b2decd4daef0463b8ed648118` and
`coexist_hw.o` sha256
`a7771f1930dc23b790c4e1c1e41c4e4208db1956777eb06d6f5bee8f1770fd71`) both
stage entry points are a bare `ret`, as the reviewed fact
`BLOB_LIBCOEX_IEEE802154_HANDOFF` records. `esp_coex_ieee802154_force_rx_enable`
calls `hal_set_IEEE802154_force_rx_enable`, which replaces `CTRL_CFG` bits
31:26 with the argument at bit 26 (`FORCE_RX_ENB`), clearing the four
multi-PAN interface enables and `RX_DONE_TRIGGER_IDLE`, and tail-calls
`hal_set_IEEE802154_coex_delay(8)`, which replaces `COEX_ARB_DELAY` (bits
23:16) with 8. The port has no external-coexistence owner, so neither write is
made; they belong with external coexistence in the coexistence driver.

The abort-enable operations are OR assignments. The selected bits above are
proven to be set, but this source set does not document reset images for those
registers, so equality to those masks must not be claimed without an additional
register-reset source or hardware observation.

The pending PIB is applied later, immediately before an operation. Its hardware
write order is frequency code, TX-power index, CCA mode, CCA threshold,
auto-ACK/enhanced-ACK flags, coordinator/promiscuous flags, then pending mode.
The frequency-code part is fully source-confirmed; the TX-power-index part is
not, because its table provider is opaque.

## 3. Channel to frequency-code mapping

The accepted channel range is inclusive `11..=26`. The value written to
`IEEE802154.CHANNEL.FREQ` is not the channel number; it is:

```text
frequency_code = (channel - 11) * 5 + 3
```

Therefore the exact mapping is:

| Channel | Code | Channel | Code |
| ---: | ---: | ---: | ---: |
| 11 | 3 | 19 | 43 |
| 12 | 8 | 20 | 48 |
| 13 | 13 | 21 | 53 |
| 14 | 18 | 22 | 58 |
| 15 | 23 | 23 | 63 |
| 16 | 28 | 24 | 68 |
| 17 | 33 | 25 | 73 |
| 18 | 38 | 26 | 78 |

This mapping is suitable for a checked channel newtype. Values outside
`11..=26` must be rejected before the register write.

## 4. `EVENT_STATUS` clearing: reviewed W1C contract

The LL helper named `ieee802154_ll_clear_events(events)` performs the volatile
bitfield RMW expression:

```c
IEEE802154.event_status.events &= events;
```

The ISR reads the current event image and passes that image to this helper
before dispatch. That sequence is consistent with write-one-to-clear hardware:
the RMW writes ones for selected events and zeroes for unselected events. It is
not consistent with ordinary RW clearing, and it would clear the wrong set on
write-zero-to-clear hardware.

The public register and struct headers alone do not label the access class.
The target's sparse reviewed fact combines the pinned LL transaction with
the bounded selective-clear observations below and assigns read-write W1C
semantics to this exact register. That fact publishes a reviewed SVD field with
`modifiedWriteValues = oneToClear`; it does not turn the register into ordinary
RW storage or authorize a caller-supplied clear mask.

The generated PAC samples the complete fourteen-bit field into an affine,
non-`Copy` token and consumes that exact token on the same-register
acknowledgement. It therefore cannot manufacture, clone, replay, or narrow a
W1C image after observation. The foundation snapshot still excludes
`EVENT_STATUS` by ownership: cold/polled acknowledgement consumes the complete
current pending image, while an active epoch separates task-owned command and
DMA registers from `Ieee802154InterruptRegisters`, whose hard-IRQ transaction
samples all event and selected sideband evidence before acknowledging the exact
snapshot.
The selected register fact retains its source and hardware provenance in
`chips/esp32s31/registers/evidence/` and `model/reviewed.toml`. Same-bit arrival and
level-line retrigger are separate hardware claims, which a hardware route
probe settled: arrivals of one latched bit coalesce, and the active route
retriggers for a later event. A selective clear observed
under detached routes does not qualify `STOP`.

## RF lifecycle dependencies

The public source delegates physical effects to `bt_bb_v2_init_cmplx`,
`register_chipv7_phy`, `phy_wakeup_init`, `phy_close_rf`,
`phy_param_track_tot`, `ieee802154_txon_delay_set` and
`bt_bb_get_tx_pwr_table`. Their public call sites establish order and ownership,
not complete register effects or calibrated RF readiness. Production
replacements belong to their reviewed PHY/HAL components; source-only MAC
configuration does not itself establish those postconditions.

PHY ownership is a client set: Wi-Fi uses bit 1, Bluetooth bit 2 and IEEE
802.15.4 bit 4. Acquisition sets only the caller bit; release clears only that
bit. Common RF enable/disable belongs to the first/last aggregate client.
Duplicate acquisition is not a per-client reference count. BTBB has a separate
first/last-user lifetime.

The PLL scheduler tracks Wi-Fi and shared Bluetooth/IEEE timestamps separately.
Immediate evaluation uses a strict elapsed-period check; the periodic callback
requests every active class. The default period is 1000 ms. Timer callbacks,
serialization, fallible hardware transitions and rollback need explicit
owners. A client bit or successful clock/reset sequence cannot mint an
RF-ready, receive-ready or transmit-ready capability.

## Sleep retention and RF gating

The ESP32-S31 powers the modem down in light sleep (`SOC_PM_SUPPORT_MODEM_PD`)
and restores modem registers by REGDMA over five PAU links, the PHY on link 4
(`SOC_PM_PAU_LINK_NUM`, `SOC_PM_PAU_REGDMA_LINK_IDX_PHY`,
`SOC_PM_MODEM_RETENTION_BY_REGDMA`, `SOC_PM_SUPPORT_REGDMA_TRIGGERED_PHY`).
The driver takes part in three ways:

- With `CONFIG_PM_ENABLE`, `ieee802154_mac_init` initializes, allocates and
  attaches the retention module `SLEEP_RETENTION_MODULE_802154_MAC` after
  allocating the interrupt, and `ieee802154_mac_deinit` detaches, frees and
  deinitializes it. The module depends on the `SLEEP_RETENTION_MODULE_BT_BB`
  and `SLEEP_RETENTION_MODULE_CLOCK_MODEM` modules. With tickless idle its one
  REGDMA entry, created at priority `REGDMA_LINK_PRI_IEEE802154`, is a
  continuous copy of the whole MAC aperture from `0x20103000` through
  `MAC_DATE` at offset `0x1a0`, 105 words.
- Unconditionally on the S31, `ieee802154_mac_init` ends with
  `esp_phy_modem_init(SLEEP_MODEM_IEEE802154)` and `ieee802154_mac_deinit`
  begins with `esp_phy_modem_deinit`, the PHY's own retention on link 4.
- With REGDMA modem retention and tickless idle, `IEEE802154_RF_DISABLE`
  closes the PHY client (`esp_phy_disable`) whenever `ieee802154_sleep`
  puts the radio to sleep, and `IEEE802154_RF_ENABLE` opens it again before
  every transmission, reception, energy detection and CCA. Without both,
  the two macros are empty and RF stays open while the radio sleeps.

The port implements none of these. It has no light-sleep entry, modem
power-down, REGDMA retention or tickless idle, so the radio's sleep is the
MAC's alone and RF stays open for the client's lifetime, as in a vendor build
without tickless idle. Three independent gates hold this:

- The pinned esp-phy-lib (`20f1db05`) lacks
  `phy_ana_i2c_master_burst_rf_onoff`, which the vendor's PHY retention link
  uses to turn RF off and on. Without it the PHY registers are not restored
  after modem power-down, so neither the PHY retention module nor light sleep
  with the modem powered down can be ported faithfully; a MAC retention
  module would have no hardware consumer.
- Closing and reopening RF around each sleep is the stop-and-start path of the
  PHY whose brownout after wake is not yet resolved.
- A standing user decision forbids closing RF for an IEEE 802.15.4 sleep until
  the pinned libphy provides that function.

The retention graph is shared: the MAC module depends on the BT_BB and modem
clock modules, which the PHY owns; Bluetooth consumes BT_BB rather than
duplicating it. While the Bluetooth LE Controller runs, a restore must not
rewrite its PHY ETM route (channel two) or the BTBB power table.

