# ESP32-S31 boot profiles

This chip binding supplies LP-fast, brownout and TRNG policy to
`oer-espressif-boot::drivers`. The dependency direction needs its own package:
profiles use both HAL register value types and family driver types; neither
chip knowledge nor HAL depends on this binding. LP-fast comes from tracked
chip/profile facts, generated with the register model's named source.

The bootstrap calls the family drivers with these profiles, retaining its
existing order, observations, waits and common deadline. HAL owns each port;
the binding owns no registers and performs no hardware initialization.
