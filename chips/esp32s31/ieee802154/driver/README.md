# ESP32-S31 IEEE 802.15.4 driver

`oer-esp32s31-ieee802154-driver` binds the Espressif MAC engine to S31 HAL
transactions. This boundary keeps HAL independent of engine orchestration.
The crate forbids unsafe code and has no PAC dependency.

`ll::S31Mac` owns HAL's `Ieee802154MacOwners`: task registers, IRQ registers
and the router's `Silent` proof. It delegates each LL entry to the same finite
HAL transaction, preserving access order. HAL performs masked pre-bind discard;
constructing the adapter does not access registers or change interrupt routing.

`engine::Ieee802154Engine` retains the MAC with frame storage claimed through
`oer_esp32s31_hal::ieee802154::frames::claim_buffers`. HAL's concrete projection
checks each frame before the driver creates the shared engine. A refused
placement returns both affine inputs. Detach/rebind keeps the same permanent
storage across quiescent MAC epochs; active operations refuse detach.

ISR service retains the existing consuming engine `Quarantined` result.
Unsupported events keep hardware and frame owners quarantined; the image
records the fault and resets. Fault values, retained record format and reset
policy remain in their existing engine/image owners.

`tx_power::ESP32S31_TX_POWER_LEVELS` selects from hardware-free chip knowledge.
The PHY HIL image composes this driver in its existing P2 polling context.
