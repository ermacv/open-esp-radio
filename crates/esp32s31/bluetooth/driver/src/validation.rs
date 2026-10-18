//! Isolated-image bridge for compiled Bluetooth hardware probes.
//!
//! This module is absent from ordinary builds. It constructs the finite HAL
//! owners inside one isolated image and immediately executes the
//! same restricted transaction used by production; no writable owner escapes
//! the Bluetooth hardware boundary.

#![deny(unsafe_code)]

pub use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerSramAddress,
    BluetoothControllerSramAddressError, BluetoothHalInitPeriod, BluetoothHalInitScale,
    BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
};

/// Task and interrupt owners of a clocked Bluetooth client in the isolated
/// image, with the arbiter of the shared registers.
fn validation_owners() -> (
    oer_esp32s31_hal::shared_radio::SharedRadio,
    crate::resources::TaskResources,
    crate::resources::InterruptBankOwner,
) {
    let (shared, partitions) =
        oer_esp32s31_hal::root::RadioHardware::for_validation().into_concurrent(());
    let clocked = oer_esp32s31_hal::bluetooth::ClockedOwner::for_validation(partitions.bluetooth);
    let (task, interrupts) = crate::resources::separate_interrupt_owner(clocked);
    (shared, task, interrupts)
}

/// Execute the exact production NRT acknowledgement transaction.
#[inline(always)]
pub fn capture_and_acknowledge_interrupts() {
    oer_esp32s31_hal::bluetooth::validation::capture_and_acknowledge_interrupts();
}

/// Take the diagnostic scheduler-BUSY sample that opens the production
/// scheduler stop sequence; `None` when it never settled within the
/// production attempt budget.
#[inline(always)]
pub fn sample_scheduler_stop_busy() -> Option<bool> {
    oer_esp32s31_hal::bluetooth::validation::sample_scheduler_stop_busy(
        crate::scheduler::DIAGNOSTIC_READ_BUDGET,
    )
    .ok()
}

/// Run the production execution modify of hardware list `index` until it
/// leaves `Pending`: zero when command one became ready, one when hardware
/// rejected it, three when a diagnostic sample never settled, `None` for an
/// index outside `0..16`.
///
/// # Safety
///
/// The caller must model the list ownership the production admission
/// requires and perform no later radio operation in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API forwards the execution-modify admission contract"
)]
#[inline(always)]
pub unsafe fn run_scheduler_execution_modify(index: u8, list_deletion: bool) -> Option<u32> {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    unsafe {
        oer_esp32s31_hal::bluetooth::validation::run_scheduler_execution_modify(
            index,
            list_deletion,
            crate::scheduler::DIAGNOSTIC_READ_BUDGET,
        )
    }
}

/// Publish the production execution lock of `address` on hardware list
/// `index` and observe it until it leaves `Pending`: zero retained, one
/// current-head reconciliation, two an unsupported hardware result, three a
/// diagnostic sample that never settled, `None` for an invalid index or
/// address.
///
/// # Safety
///
/// The caller must model the merge-selected item and list serialization the
/// production publication requires and perform no later radio operation in
/// this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API forwards the execution-lock publication contract"
)]
#[inline(always)]
pub unsafe fn run_scheduler_execution_lock(address: u32, index: u8) -> Option<u32> {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    unsafe {
        oer_esp32s31_hal::bluetooth::validation::run_scheduler_execution_lock(
            address,
            index,
            crate::scheduler::DIAGNOSTIC_READ_BUDGET,
        )
    }
}

/// Execute the exact production scheduler hardware-list head clear transaction.
#[inline(always)]
pub fn clear_scheduler_hardware_list_heads() {
    let (shared, mut task, interrupts) = validation_owners();
    let cleared_lists = task.clear_scheduler_hardware_list_heads();
    // The comparison image deliberately retains the mutated partitions; it
    // must not reconstruct cold ownership without the missing rollback.
    let _powered_owners = (shared, task, interrupts, cleared_lists);
}

/// Execute the production PHY-I2C host selection through the shared radio
/// arbiter's PHY borrow, as the Bluetooth client reaches it.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub fn configure_and_select_phy_i2c_host(block: u8) -> u32 {
    let (shared, task, interrupts) = validation_owners();
    let host = {
        let Ok(mut lease) = shared.try_acquire() else {
            unreachable!("the isolated image holds no other lease");
        };
        let mut shared_phy = lease.phy_hal();
        oer_esp32s31_phy::validation::configure_and_select_phy_i2c_host(&mut shared_phy, block)
    };
    // Host selection mutates shared PHY state. The isolated comparison image
    // retains every partition and must not advertise a reusable cold owner.
    let _powered_owners = (shared, task, interrupts);
    host
}

/// Execute the exact finite MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)` with the reviewed linked `phy_param` byte.
///
/// This validation-only bridge cannot bypass the production Bluetooth
/// lifecycle in an ordinary build because it is absent there. In
/// production the first BTBB holder runs it when the Bluetooth client joins
/// the registered shared PHY domain, which projects this byte from the
/// registration that describes the lease.
///
/// # Safety
///
/// The caller must be an isolated compiled-production probe that models a
/// target-registered common-PHY transition with a settled Bluetooth client and
/// completed initial tracking. It must not use another radio owner after this
/// call because the powered owners remain retained until verified teardown
/// exists.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the settled Bluetooth PHY-client prerequisite"
)]
#[inline(always)]
pub unsafe fn initialize_baseband_v2(gain_parameter: u8) {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract,
    // which states the lower transaction's prerequisites.
    unsafe {
        oer_esp32s31_hal::bluetooth::validation::initialize_baseband_v2(gain_parameter);
    }
}

/// Execute the complete production BTDM controller HAL-init component with
/// the exact standalone profile recovered from the pinned controller caller.
///
/// # Safety
///
/// The caller must be an isolated compiled-production probe modeling enabled
/// clocks, the selected controller-SRAM prefix and an inactive IRQ bank. The
/// powered owners remain retained after return; scheduler, PHY, BTBB,
/// Link-Layer and HCI readiness remain unclaimed.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the complete HAL-init prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_controller_hal_reviewed_standalone() {
    let (shared, mut task, interrupts) = validation_owners();
    // SAFETY: forwarded unchanged from this function's `# Safety` contract,
    // which states the lower transaction's prerequisites.
    unsafe {
        task.initialize_controller_hal(BluetoothControllerHalInitConfig::reviewed_standalone());
    }
    let _powered_owners = (shared, task, interrupts);
}

/// Apply the exact modem low-power timer register prefix before source 127.
///
/// This isolated path compiles the shipping opaque HAL transition while
/// retaining both terminal owners. It does not publish ISR storage, allocate
/// the CPU route, or claim that the controller software environment exists.
///
/// # Safety
///
/// The caller must model enabled controller/timer clocks and completed task,
/// HAL, scheduler/event-list and HCI software initialization. No later radio
/// operation may execute in the comparison image.
#[allow(
    unsafe_code,
    reason = "the isolated validation image assumes the missing software-stage prerequisite"
)]
#[inline(always)]
pub unsafe fn prepare_modem_lp_timer_registers() {
    let (shared, partitions) =
        oer_esp32s31_hal::root::RadioHardware::for_validation().into_concurrent(());
    let (mut task, interrupts) =
        oer_esp32s31_hal::bluetooth::ClockedOwner::for_validation(partitions.bluetooth)
            .separate_interrupt_owner();
    // SAFETY: forwarded unchanged from this function's `# Safety` contract,
    // which states the lower transaction's prerequisites.
    let prepared = unsafe { task.prepare_modem_lp_timer_registers() }
        .expect("a fresh Bluetooth task retains the modem LP-timer partition");
    let _terminal_owners = (shared, task, prepared, interrupts);
}

/// Execute one exact production memory-list pointer publication in an
/// isolated comparison image.
///
/// # Safety
///
/// The caller must satisfy the controller lifecycle, serialization, backing
/// storage, initialization, and lifetime prerequisites documented by the PAC
/// validation bridge. No later radio operation may run in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves controller-list ownership prerequisites"
)]
#[inline(always)]
pub unsafe fn program_memory_list_pointer(
    selector: BluetoothMemoryListSelector,
    slot: BluetoothMemoryListSlot,
    image: BluetoothMemoryListPointerImage,
) {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract,
    // which states the lower transaction's prerequisites.
    unsafe {
        oer_esp32s31_hal::bluetooth::validation::program_memory_list_pointer(selector, slot, image);
    }
}

/// Execute the complete recovered BLE PHY register-init body in an isolated
/// compiled-production comparison image.
///
/// `false` means one supplied address did not satisfy its observed encoding
/// contract and no MMIO was performed.
///
/// # Safety
///
/// The caller must satisfy the complete lifecycle and backing-storage
/// prerequisites documented by the PAC validation bridge. After a successful
/// call it must not perform another radio operation in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the complete BLE PHY lifecycle prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_phy_registers(
    private_timing_source_byte: u8,
    environment_address: u32,
    resolving_list_address: u32,
    set_branch_control_0470_bit_18: bool,
    configuration_word_40: u32,
) -> bool {
    let Ok(resolving_list) = BluetoothControllerSramAddress::new(resolving_list_address) else {
        return false;
    };
    // SAFETY: forwarded unchanged from this function's `# Safety` contract,
    // which states the lower transaction's prerequisites.
    unsafe {
        oer_esp32s31_hal::bluetooth::validation::initialize_phy_registers(
            private_timing_source_byte,
            environment_address,
            resolving_list,
            set_branch_control_0470_bit_18,
            configuration_word_40,
        )
    }
}

/// Run the production Bluetooth low-power timer clock selection once, on
/// isolated validation owners of the shared radio. Returns whether the lease
/// accepted it.
#[inline(always)]
pub fn select_low_power_clock() -> bool {
    oer_esp32s31_hal::bluetooth::validation::select_low_power_clock()
}

/// Run the production Bluetooth low-power timer clock deselection
/// transaction once, on isolated validation owners of the shared radio.
#[inline(always)]
pub fn deselect_low_power_clock() {
    oer_esp32s31_hal::bluetooth::validation::deselect_low_power_clock();
}
