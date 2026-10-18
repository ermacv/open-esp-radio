//! Exact bounded BLE PHY register-initialization transaction.
//!
//! The recovered vendor lifecycle performs this transaction only after common
//! PHY, Bluetooth baseband, coexistence and controller software initialization
//! have completed. Possessing [`BluetoothTaskRegisters`] alone proves none of
//! those prerequisites, so the operation remains unsafe and is reached by the
//! affine controller lifecycle through the HAL.

#![deny(unsafe_code)]

use crate::{
    BluetoothControllerSramAddress, BluetoothTaskRegisters, SharedRadioRegisters, device_fence,
};

/// Environment member whose raw address the transaction publishes at
/// `0x201018a0`.
const ENVIRONMENT_MEMBER_OFFSET: u32 = 0x2c;
/// Last environment member, whose raw address the transaction publishes at
/// `0x2010124c`.
const ENVIRONMENT_LAST_OFFSET: u32 = 0x4d;
/// `r_ble_controller_init` writes halfword 91 to controller configuration
/// `+0x10` over the rodata default 90; CCA, which would override it, is off.
const NORMAL_PROFILE_PRIVATE_TIMING_SOURCE_BYTE: u8 = 91;
const NORMAL_PROFILE_SET_BRANCH_CONTROL_0470_BIT_18: bool = false;
/// Controller configuration `+0x40`: the rodata default of
/// `sym_controller_jqcSm1kAtUzAMoyGlaKq`, which nothing later writes.
const NORMAL_PROFILE_CONFIGURATION_WORD_40: u32 = 0x7d0;
/// Low byte of the complete `0x20101b74` image `0x1ba`.
const RUNTIME_CONFIGURATION_LOW_BYTE: u8 = 0xba;

trait BleBaseStackOnTaskEnableHardwareTransaction {
    fn enable_access_address_low_correlation(&mut self);
    fn initialize_ble_phy_registers(&mut self);
}

fn execute_base_stack_on_task_enable_hardware(
    transaction: &mut impl BleBaseStackOnTaskEnableHardwareTransaction,
) {
    transaction.enable_access_address_low_correlation();
    transaction.initialize_ble_phy_registers();
}

struct HardwareBleBaseStackOnTaskEnableTransaction<'registers> {
    registers: &'registers mut BluetoothTaskRegisters,
    shared: &'registers mut SharedRadioRegisters,
    inputs: BluetoothPhyRegisterInitInputs,
}

impl BleBaseStackOnTaskEnableHardwareTransaction
    for HardwareBleBaseStackOnTaskEnableTransaction<'_>
{
    fn enable_access_address_low_correlation(&mut self) {
        crate::generated::enable_ble_phy_access_address_low_correlation(
            &self.shared.shared_radio.bt_v3_2_baseband,
        );
    }

    #[allow(
        unsafe_code,
        reason = "the composed transaction retains the exact PHY leaf prerequisites"
    )]
    fn initialize_ble_phy_registers(&mut self) {
        // SAFETY: the enclosing composed transaction carries the same
        // lifecycle and pointed-storage prerequisites as the exact PHY leaf.
        unsafe {
            self.registers
                .initialize_ble_phy_registers(self.shared, self.inputs);
        }
    }
}

/// Typed base of the linked BLE PHY environment.
///
/// This proves only word alignment and representability of the last published
/// member. Allocation, contents, lifetime and hardware ownership belong to the
/// controller-memory and lifecycle layers above the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyEnvironmentAddress(u32);

/// Why a BLE PHY environment address cannot be represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPhyEnvironmentAddressError {
    /// The environment's word members would be misaligned.
    Unaligned,
    /// The final published member would overflow the address space.
    ExtentOverflow,
}

impl BluetoothPhyEnvironmentAddress {
    /// Validate the positional address-image requirements without dereference.
    pub const fn new(address: u32) -> Result<Self, BluetoothPhyEnvironmentAddressError> {
        if !address.is_multiple_of(4) {
            return Err(BluetoothPhyEnvironmentAddressError::Unaligned);
        }
        if address.checked_add(ENVIRONMENT_LAST_OFFSET).is_none() {
            return Err(BluetoothPhyEnvironmentAddressError::ExtentOverflow);
        }
        Ok(Self(address))
    }

    /// Return the validated CPU address without granting dereference.
    pub const fn address(self) -> u32 {
        self.0
    }
}

/// Complete external inputs read by the recovered BLE PHY init body.
///
/// The timing source remains private Controller policy. Values whose hardware
/// meaning is not proven retain positional configuration names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyRegisterInitInputs {
    private_timing_source_byte: u8,
    environment: BluetoothPhyEnvironmentAddress,
    resolving_list: BluetoothControllerSramAddress,
    set_branch_control_0470_bit_18: bool,
    configuration_word_40: u32,
}

impl BluetoothPhyRegisterInitInputs {
    /// Bind the source-owned normal Controller profile to its two live SRAM
    /// objects.
    ///
    /// The returned value contains the complete positional policy recovered for
    /// ordinary production use. Callers supply only typed addresses; the PAC
    /// retains all register-facing profile choices. This value does not itself
    /// prove allocation, contents, lifetime, or exclusive ownership of either
    /// object.
    pub const fn normal_controller_profile(
        environment: BluetoothPhyEnvironmentAddress,
        resolving_list: BluetoothControllerSramAddress,
    ) -> Self {
        Self {
            private_timing_source_byte: NORMAL_PROFILE_PRIVATE_TIMING_SOURCE_BYTE,
            environment,
            resolving_list,
            set_branch_control_0470_bit_18: NORMAL_PROFILE_SET_BRANCH_CONTROL_0470_BIT_18,
            configuration_word_40: NORMAL_PROFILE_CONFIGURATION_WORD_40,
        }
    }

    /// Capture arbitrary positional inputs for an isolated vendor-comparison
    /// image.
    ///
    /// Shipping callers cannot select these register-facing values. The sole
    /// crate-local consumer is the validation bridge that compares the bounded
    /// PAC transaction against externally seeded vendor state.
    #[cfg(feature = "validation-probes")]
    pub(crate) const fn validation_profile(
        private_timing_source_byte: u8,
        environment: BluetoothPhyEnvironmentAddress,
        resolving_list: BluetoothControllerSramAddress,
        set_branch_control_0470_bit_18: bool,
        configuration_word_40: u32,
    ) -> Self {
        Self {
            private_timing_source_byte,
            environment,
            resolving_list,
            set_branch_control_0470_bit_18,
            configuration_word_40,
        }
    }
}

/// Owner of a disabled BLE PHY ETM route. Only
/// [`BluetoothTaskRegisters::restore_ble_phy_etm_route`] consumes it, so a
/// path that loses it leaves visible evidence rather than a silently
/// unrouted PHY.
#[derive(Debug)]
#[must_use = "a disabled BLE PHY ETM route must be restored"]
pub struct BlePhyEtmRouteDisabled {
    _private: (),
}

impl BluetoothTaskRegisters {
    /// Disable the BLE PHY ETM route for one Direct Test Mode event.
    ///
    /// SOURCE: pinned `libble_app.a[dtm_4.o]` event bodies
    /// `sym_dtm_2zeOUjc7g55zkDNQuZkg` and
    /// `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` disable, for a test without CTE,
    /// the vendor channel carrying the route that PHY register initialization
    /// installs; this owner carries that route on channel two.
    pub fn disable_ble_phy_etm_route(&mut self) -> BlePhyEtmRouteDisabled {
        self.etm.disable();
        BlePhyEtmRouteDisabled { _private: () }
    }

    /// Route and enable the BLE PHY ETM channel again after a test.
    ///
    /// SOURCE: pinned Test End `sym_dtm_NsbldBIeGraE2wg0AVy7` calls
    /// `r_ble_phy_init`, which leads to the PHY register initialization
    /// that routes and enables the channel.
    pub fn restore_ble_phy_etm_route(&mut self, route: BlePhyEtmRouteDisabled) {
        let BlePhyEtmRouteDisabled { _private: () } = route;
        self.etm.route_and_enable();
    }

    /// Execute the complete hardware component of BLE base-stack task enable.
    ///
    /// The registered external-baseband callback first enables access-address
    /// low-correlation handling through one fresh field RMW. The exact BLE PHY
    /// register-initialization body follows without an intervening MMIO edge.
    /// Source-owned memory tracking replaces the preceding vendor software
    /// publication; the standalone profile does not enter the optional BQB
    /// branch.
    ///
    /// Every read-modify-write below performs a fresh volatile read. In
    /// particular, repeated updates to `0x20101650`, `0x20101654`, and the four
    /// byte-image words intentionally do not carry a prior software image
    /// across operations.
    ///
    /// The transaction has no rollback edge. A higher lifecycle layer must
    /// treat failure after its first write as fail-stop until a complete
    /// controller teardown is independently recovered and verified.
    ///
    /// # Safety
    ///
    /// The caller must prove the complete pre-task-enable lifecycle: common
    /// PHY and Bluetooth baseband are initialized, an exclusive standalone or
    /// shared-radio coexistence policy is retained, source-owned Controller,
    /// HCI and scheduler state exists, the IRQ owner remains inactive, and
    /// both pointed-to SRAM objects remain correctly initialized, exclusively
    /// serialized and live for every hardware consumer. This method does not
    /// establish or verify any of those facts.
    #[allow(
        unsafe_code,
        dead_code,
        reason = "the unsafe signature retains unmodeled lifecycle and pointed-storage prerequisites"
    )]
    pub unsafe fn enable_ble_base_stack_hardware(
        &mut self,
        shared: &mut SharedRadioRegisters,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        let mut transaction = HardwareBleBaseStackOnTaskEnableTransaction {
            registers: self,
            shared,
            inputs,
        };
        execute_base_stack_on_task_enable_hardware(&mut transaction);
    }

    /// Execute the exact bounded BLE PHY register-initialization body.
    ///
    /// This leaf starts with the ordered positional-word publications. It does
    /// not perform the preceding base-stack access-address correlation update;
    /// [`Self::enable_ble_base_stack_hardware`] owns that composition.
    ///
    /// # Safety
    ///
    /// The caller must prove completed common PHY and Bluetooth baseband
    /// initialization, the registered external-baseband table used by this
    /// body, inactive IRQ ownership, and live exclusive ownership of both
    /// pointed-to SRAM objects for every hardware consumer. After the first
    /// MMIO write, failure must be treated as fail-stop until a complete
    /// controller teardown is independently recovered and verified.
    #[allow(
        unsafe_code,
        dead_code,
        reason = "the unsafe signature retains the exact PHY leaf lifecycle and storage prerequisites"
    )]
    pub unsafe fn initialize_ble_phy_registers(
        &mut self,
        shared: &mut SharedRadioRegisters,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        let timing_byte = inputs.private_timing_source_byte.wrapping_sub(1);
        let environment = inputs.environment.address();
        let environment_member = environment + ENVIRONMENT_MEMBER_OFFSET;
        let environment_tail = environment + ENVIRONMENT_LAST_OFFSET;
        let resolving_list = inputs.resolving_list.compressed_image();

        let bluetooth = &self.bluetooth;
        let btmac = &bluetooth.btmac_ble_phy_init;

        // Ordered positional images and BTMAC prefix.
        crate::svd::zero_register_write::publish_ble_positional_word_1ff0_image_0(
            &bluetooth.ble_hw_positional_word_1ff0,
        );
        crate::svd::fixed_register_image::publish_ble_positional_word_1ff0_image_1(
            &bluetooth.ble_hw_positional_word_1ff0,
        );
        crate::svd::sampled_bit_zero_write::preserve_ble_phy_interrupt_source_17(btmac);
        crate::svd::fixed_register_image::clear_all_ble_base_stack_interrupt_sources(btmac);

        // `r_ble_phy_ramup_time_set`: the TX-on delay fields, then the
        // external-baseband LE TX-on delay it tail-calls.
        crate::generated::clear_ble_phy_lc_tx_on_delay_fields(btmac);
        crate::generated::or_ble_phy_init_tx_on_delay(
            btmac,
            crate::generated::BluetoothPhyInitTimingByte::new(u32::from(timing_byte))
                .expect("one byte always fits the reviewed BLE PHY timing domain"),
        );
        crate::generated::publish_ble_phy_le_tx_on_delay(
            &shared.shared_radio.bt_v3_2_baseband,
            crate::generated::BluetoothPhyInitTimingByte::new(u32::from(
                timing_byte.wrapping_sub(10),
            ))
            .expect("one byte always fits the reviewed BLE PHY timing domain"),
        );

        crate::svd::fixed_register_image::publish_ble_phy_init_value_0138(btmac);
        crate::svd::fixed_register_image::publish_ble_phy_init_bytes_04a4(btmac);
        crate::svd::fixed_register_image::publish_ble_phy_init_bytes_04a8(btmac);
        crate::svd::register_image_write::publish_ble_phy_init_environment_member(
            btmac,
            environment_member,
        );
        crate::svd::fixed_register_image::publish_ble_phy_init_value_04ac(btmac);
        crate::svd::fixed_register_image::publish_ble_phy_init_value_045c(btmac);

        // Four independent fresh-read updates at 0x20101654; the last two
        // are `r_ble_phy_monitor_bb_sync`, which then clears 0x20101474 and
        // publishes image 0x20 at 0x2010891c.
        crate::generated::clear_ble_phy_init_low_byte_pair(btmac);
        crate::generated::or_ble_phy_init_low_byte_pair(btmac);
        crate::generated::clear_ble_phy_init_byte_2_low_7(btmac);
        crate::generated::or_ble_phy_init_byte_2(btmac);

        crate::svd::zero_register_write::clear_ble_phy_init_zero_0074(btmac);
        crate::svd::fixed_register_image::publish_ble_positional_word_891c_image_20(
            &bluetooth.ble_hw_positional_word_891c,
        );
        crate::svd::fixed_register_image::publish_ble_phy_accelerator_config(
            &bluetooth.ble_hw_accelerator,
        );
        crate::svd::fixed_register_image::publish_ble_phy_accelerator_sram_region_0(
            &bluetooth.ble_hw_accelerator,
        );
        crate::svd::fixed_register_image::publish_ble_phy_accelerator_sram_region_1(
            &bluetooth.ble_hw_accelerator,
        );
        crate::svd::zero_based_field_write::publish_ble_phy_resolving_list_base_pointer(
            &bluetooth.ble_hw_resolving_list,
            resolving_list,
        );

        crate::svd::fixed_register_image::publish_ble_phy_init_control_0400(btmac);
        crate::generated::enable_ble_phy_init_control_0400(btmac);
        crate::svd::register_image_write::publish_ble_phy_init_configuration_word(
            btmac,
            inputs.configuration_word_40,
        );

        // Each byte replacement is a distinct fresh-read RMW in vendor order.
        crate::generated::publish_ble_phy_init_0550_byte_0(btmac);
        crate::generated::publish_ble_phy_init_0550_byte_1(btmac);
        crate::generated::publish_ble_phy_init_0550_byte_2(btmac);

        crate::generated::publish_ble_phy_init_0554_byte_0(btmac);
        crate::generated::publish_ble_phy_init_0554_byte_1(btmac);
        crate::generated::publish_ble_phy_init_0554_byte_2(btmac);
        crate::generated::publish_ble_phy_init_0554_byte_3(btmac);

        crate::generated::publish_ble_phy_init_055c_byte_0(btmac);
        crate::generated::publish_ble_phy_init_055c_byte_1(btmac);
        crate::generated::publish_ble_phy_init_055c_byte_2(btmac);
        crate::generated::publish_ble_phy_init_055c_byte_3(btmac);

        crate::generated::publish_ble_phy_init_0558_byte_0(btmac);
        crate::generated::publish_ble_phy_init_0558_byte_1(btmac);
        crate::generated::publish_ble_phy_init_0558_byte_2(btmac);
        crate::generated::publish_ble_phy_init_0558_byte_3(btmac);

        crate::generated::publish_ble_phy_init_high_half_0458(btmac);
        crate::generated::publish_ble_phy_init_low_5_054c(btmac);

        if inputs.set_branch_control_0470_bit_18 {
            crate::generated::set_ble_phy_device_table_control_bit_18(btmac);
        }
        crate::svd::fixed_register_image::publish_ble_positional_word_891c_image_40(
            &bluetooth.ble_hw_positional_word_891c,
        );

        crate::svd::zero_based_field_write::publish_ble_phy_runtime_configuration(
            &bluetooth.ble_hw_runtime_control,
            RUNTIME_CONFIGURATION_LOW_BYTE,
            true,
        );
        crate::svd::fixed_register_image::publish_ble_phy_init_followup_image_1(
            &bluetooth.ble_hw_runtime_control,
        );

        let controller = &bluetooth.bluetooth_controller_core;
        crate::svd::zero_register_write::clear_ble_phy_controller_value_0244(controller);
        crate::svd::fixed_register_image::publish_ble_phy_controller_value_01f0(controller);
        crate::svd::fixed_register_image::publish_ble_phy_controller_value_0248(controller);
        // The vendor routes and enables channel zero; this owner's channel
        // two carries the same route (see `BluetoothPhyEtmChannel`).
        self.etm.route_and_enable();
        crate::svd::register_image_write::publish_ble_phy_controller_environment_tail(
            controller,
            environment_tail,
        );

        // One ordering boundary is a reviewed Rust-side addition. It does not
        // replace, merge, or reorder any vendor MMIO edge above.
        device_fence();
    }
}

#[cfg(test)]
mod tests;
