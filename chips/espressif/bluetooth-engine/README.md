# BLE engine contracts

`oer-espressif-bluetooth-engine` is a hardware-free family contract, with no
chip, HAL or PAC dependency. It forbids unsafe code and runtime panic paths.
The S31 HAL implements its ports on the owners that already hold the physical
parts; the S31 Bluetooth driver's controller-time worker uses the same port
with its live engine and recording fakes.

`SchedulerTaskPort` carries validated command requests, finite list indices,
BUSY observations and backend-owned affine publication/stopped receipts.
Clearing a command consumes its matching publication receipt. The separate
`SchedulerInterruptPort` exposes P3 diagnostic selection, one fresh sample
and the existing bank-mask atom. It neither transfers that owner nor adds a
shared writer or lock. `ControllerTimeRegisters` describes the physical
publication/sample edges; `ControllerTimePort` preserves sticky in-flight
ownership across cancellation.

`OutputTaskPort` and `OutputInterruptPort` carry HAL's existing affine release
phases through the alternating task/handler transactions. No released bank
or arena can be extracted through these ports.

`ExecutionModifyPhase` and `StopPhase` are durable progress values, without
hardware authority. `EngineEpoch` is a non-wrapping installation identity;
a detached numeric identity cannot create a completion receipt. Exact arena,
pool and epoch brands and all receipt constructors remain private to HAL's
memory/owner core. No port exports a raw address, graph projection, arena
extraction or reclamation operation.

HAL still executes execution-lock/modify/stop, diagnostic settling, interrupt
request/reply exchange, admission/terminal release and the role workers.
Their waits, decisions, source values, register/fence order, absorbing
quarantine and reset obligations are unchanged. Terminal release still
admits sampled NRT before the last visible fault check, excludes fault lanes
from unconditional W1C and verifies closed enables without reopening routes.
