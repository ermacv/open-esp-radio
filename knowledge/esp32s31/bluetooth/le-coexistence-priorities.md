# ESP32-S31 LE coexistence priorities

Source: `esp32s31-bt-lib` `libble_app.a` (pin `10c50778`), members
`coexAdv.c.o_1.o` (`r_sym_coexAdv_pNmEzY32xPoYRR8VuoXP`, role
`r_ble_lll_adv_coex_pti_init`; `r_sym_coexAdv_VG7vTjgdtb9y55QZyPP0`, role
`r_ble_lll_adv_coex_dpc_update_on_adv_start`;
`r_sym_coexAdv_A5DXuOYpsXb125NuYN3d`; `r_sym_coexAdv_sLW7oGzvK2Nq64ivPWK2`;
table `sym_coexAdv_H8ZFnQsVsYzrsw6Rv2Wu`), `coexConn.c.o_1.o`
(`r_sym_coexConn_BqYvJSQvD3IUJJyL5jD8`, role `r_ble_lll_conn_coex_pti_init`;
`r_sym_coexConn_zaMnJdgVnBd25ru35IkB`, role
`r_ble_lll_conn_coex_dpc_update_on_event_scheduled`;
`r_sym_coexConn_FwW1rUBEXrFbsmQm3dvU`; `r_sym_coexConn_8zwo11wSByltviHFU8eN`;
`r_sym_coexConn_sQLo...`; `r_sym_coexConn_H75Y...`; table
`sym_coexConn_1jwNCSpbwgr6k9BEu0Eg`), `coexScan.c.o_1.o`
(`r_sym_coexScan_FGUQNnreeyQiPkw2qTYu`, `r_sym_coexScan_s7w1EV32meG8f6y0sPBq`,
`r_sym_coexScan_wNFqQvVjWMhmRY8ZGk4o`) and `ble_2.o`
(`r_sym_ble_2eLkvpKuoaxT83Mj406p`); extracted from
the predecessor repository's `crates/bluetooth/le/controller/src/coexistence.rs`.

Facts and decision structure only. Every threshold, period, count and lane
value is to be measured on hardware.

## Layout

- Scheduler item `+0x24` bits 19:0 hold four five-bit radio-request priority
  lanes; lane `i` is bits `5i+4:5i`.
- Advertising and passive-scan items use all four lanes. A passive scanner's
  lanes two and three are zero.
- A connection item uses two lanes: lane zero is the event's own priority,
  lane one the connection's base priority.
- Connection link state `+0x30` bits 19:14 hold a six-bit protection time in
  units of 256 us; bits 21:20 are set while it is in force.
- Alone on the antenna every lane requests 15 and no protection is set; this
  equal standalone policy is our product choice.

## Levels

Each event has a level, baseline or raised. With another radio on the
antenna the vendor writes lane zero of each event from a per-role table
indexed by that level; the other lanes come from the role's default lane
table at allocation.

## Advertising rule

- The advertising interval falls into one of three bands, bounded by two
  thresholds in units of 625 us. Each band selects a period N: one event in
  every N is raised, with N decreasing as the interval grows (the longest
  band raises every event).
- `r_sym_coexAdv_A5DX...` counts one update per ended event and raises the
  level when the count is a multiple of N. Starting the set raises the level
  until the first update.
- `r_sym_ble_2eLkvpKuoaxT83Mj406p` schedules the next event before that
  update, so an event carries the level of the count before the previous
  event ended; the first two events carry the start level.

## Connection rule

- The first K events of a connection are raised.
- Afterwards the level is raised when more events have passed since the event
  of the last valid reception than the interval allows. The allowance comes
  from interval bands (thresholds in units of 625 us); longer intervals allow
  fewer missed events, and beyond the last band none.
- A local control procedure raises the level from when it starts
  (`r_sym_coexConn_FwW1...`).
- `r_sym_coexConn_8zwo...` counts receptions from event zero when the
  connection is created.
- The vendor raises a central's level to the highest level after repeated
  failed events; a peripheral's is never raised that way.
- `r_sym_coexConn_H75Y...` sets the protection for a number of 625 us slots,
  converted to 256 us units.

## To be measured

The band thresholds, the advertising periods, K, the allowed missed-event
counts, the protection slot count and every lane value. Methods:

- Lanes in use: dump scheduler item `+0x24` from a running vendor image under
  shared-radio load.
- Thresholds: they show only indirectly, through which events are raised.
- coexHook bits: read the coex status on hardware.

Every measured value must name its hardware run.
