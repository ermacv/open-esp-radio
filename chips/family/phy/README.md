# Family PHY driver

Safe PHY operations over chip-owned register ports, with no dependency on a
chip, PAC, executor or terminal owner:

- `analog_i2c`: RF/baseband power order and the conditional analog reset pulse.
- `serial`: routed byte starts and completion samples; busy or unrouted starts
  publish nothing. Reads preserve host-map → read-mask → command order and
  sample data only after a non-busy observation.
- `i2c`: finite parallel-host initialization with a caller-supplied chip profile
  and cooperative wait. The normal map is restored on success and finite error;
  cancellation retains the current hardware state with its outer owner.
- `pbus`: debug/work-mode ordering and one completion observation before clear.
- `settle`: named minimum machine-cycle settles from 1 through 20 microseconds,
  using the CPU rate already adopted by the clock owner. Wrapping samples and
  interrupt preemption preserve the minimum; interrupts remain enabled.

The S31 binding owns the retained-wake pair profile. The HAL supplies atom ports,
fixed vendor write series, exclusive register custody and restore backing.
Registration, SAR transfer, exact consuming reunion, quarantine and cancellation
remain obligations of those affine owners. The family driver owns no fatal slot
or reset callback. Calibration and tracking orchestration retain their current
S31 bindings.

Host regressions record port edges and drive independent waits with
`oer-time-virtual`. These tests establish operation order and finite failure
behavior, not hardware timing or radio qualification.
