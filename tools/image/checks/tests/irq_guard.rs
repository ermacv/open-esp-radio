// Exercise the runtime's exact NAPOT encoding without an ESP target.
use oer_espressif_staged_runtime::BootError;

#[path = "../../../../chips/espressif/staged-runtime/src/stacks/guard.rs"]
mod guard;
