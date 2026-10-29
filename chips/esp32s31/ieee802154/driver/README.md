# ESP32-S31 IEEE 802.15.4 driver

`oer-esp32s31-ieee802154-driver` binds the Espressif MAC engine to S31 HAL
transactions. This boundary keeps HAL independent of engine orchestration.
The crate forbids unsafe code and has no PAC dependency.

`ll::S31Mac` owns HAL's `Ieee802154MacOwners`: task registers, IRQ registers
and the router's `Silent` proof. It delegates each LL entry to the same finite
HAL transaction, preserving access order. `lifecycle` binds the family's reset,
masked startup-discard and sampled teardown to HAL's consuming phase ports;
constructing the adapter does not access registers or change interrupt routing.

The family engine retains reset/foundation validation and static policy. HAL
retains module-clock/partition ownership and reset loans, without readback
decisions. The reset loan is associated with its HAL backend; the
PHY operational route retains the family's affine foundation-return proof.
Startup phases expose neither an IRQ extraction nor general
unsampled clearing: mask, abort baseline, STOP, both timer stops, discard of
all fourteen lanes and the final fence precede bindable ownership. Bound
activation and later sessions publish the same ten runtime events. Polling
requires `Silent`; teardown carries its saved affine sample through masking.

`engine::Ieee802154Engine` retains the MAC with frame storage claimed through
`oer_esp32s31_hal::ieee802154::frames::claim_buffers`. HAL's concrete projection
checks each frame before the driver creates the shared engine. A refused
placement returns both affine inputs. Detach/rebind keeps the same permanent
storage across quiescent MAC epochs; active operations refuse detach.

The driver's consuming `isr` calls the family owner's borrowed `service`.
Unsupported events retain their complete observation and entry state in `Fault`,
while the driver keeps the MAC and placed DMA storage in absorbing `Quarantined`
custody. Neither dropping quarantine nor reading its fault releases an owner;
there is no detach, rebind or operational access. Explicit mutable entries keep
the family service inaccessible through the driver's shared `Deref`.
The PHY HIL image owns the 44-byte retained codec, its storage and the
safe `fatal::reset_recording` call. The driver does not record or reset.

`tx_power::ESP32S31_TX_POWER_LEVELS` selects from hardware-free chip knowledge.
The PHY HIL image composes this driver in its existing P2 polling context.
