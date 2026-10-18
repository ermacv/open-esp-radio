# HIL system watchdog

The dedicated `system-watchdog` image uses the SoC deadline watchdog (TIMG1)
with a 1 s test budget. It starts neither radio nor a network stack,
advertises `system_watchdog`, and reports reset cause through
`GetBootStatus`. It needs only the DUT and records reset evidence, not RF-stop
timing.
