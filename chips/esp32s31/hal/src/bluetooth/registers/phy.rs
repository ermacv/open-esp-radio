//! Exact bounded BLE PHY register-initialization transaction.
//!
//! The recovered vendor lifecycle performs this transaction only after common
//! PHY, Bluetooth baseband, coexistence and controller software initialization
//! have completed. Possessing [`BluetoothTaskRegisters`] alone proves none of
//! those prerequisites, so the operation remains unsafe and is reached by the
//! affine controller lifecycle through the HAL.

#![deny(unsafe_code)]

use crate::layouts::{BlePhyEnvironment, BleResolvingList, DmaRef};

use oer_esp32s31_pac::{bt_bb::BtV32Baseband, modem_etm::ModemEtmBluetoothPhy};
use oer_mmio::Placement;

use crate::bluetooth::registers::{BluetoothPlacements, BluetoothTaskRegisters, device_fence};

/// `r_ble_controller_init` writes halfword 91 to controller configuration
/// `+0x10` over the rodata default 90; CCA, which would override it, is off.
const NORMAL_PROFILE_PRIVATE_TIMING_SOURCE_BYTE: u8 = 91;
const NORMAL_PROFILE_SET_BRANCH_CONTROL_0470_BIT_18: bool = false;
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

struct HardwareBleBaseStackOnTaskEnableTransaction<'registers, P, B>
where
    P: BluetoothPlacements,
    B: Placement,
{
    registers: &'registers mut BluetoothTaskRegisters<P>,
    baseband: &'registers mut BtV32Baseband<B>,
    inputs: BluetoothPhyRegisterInitInputs,
}

impl<P, B> BleBaseStackOnTaskEnableHardwareTransaction
    for HardwareBleBaseStackOnTaskEnableTransaction<'_, P, B>
where
    P: BluetoothPlacements,
    B: Placement,
{
    fn enable_access_address_low_correlation(&mut self) {
        self.baseband.modify_le_correlation_threshold_0(|w| {
            w.set_aa_lc_enable(true);
        });
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
                .initialize_ble_phy_registers(self.baseband, self.inputs);
        }
    }
}

/// Route modem event 8 to task 20 on channel two, the event first, and
/// enable the channel.
///
/// The event and task identities are the vendor's whole-word images: the
/// model's value lists name only the IEEE 802.15.4 identities, and each
/// identity field is its word's one 32-bit field.
///
/// SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS.
fn route_and_enable<E: Placement>(etm: &mut ModemEtmBluetoothPhy<E>) {
    etm.modify_ch2_event(|w| {
        w.set_id(oer_esp32s31_pac::modem_etm::ch_event::Id::BlePhyEvent08.bits());
    });
    etm.modify_ch2_task(|w| {
        w.set_id(oer_esp32s31_pac::modem_etm::ch_task::Id::BlePhyTask14.bits());
    });
    etm.trigger_channel_enable_set_ch2();
}

/// Complete external inputs read by the recovered BLE PHY init body.
///
/// The timing source remains private Controller policy. Values whose hardware
/// meaning is not proven retain positional configuration names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyRegisterInitInputs {
    private_timing_source_byte: u8,
    environment: DmaRef<BlePhyEnvironment>,
    resolving_list: DmaRef<BleResolvingList>,
    set_branch_control_0470_bit_18: bool,
}

impl BluetoothPhyRegisterInitInputs {
    /// Bind the source-owned normal Controller profile to its two live SRAM
    /// objects.
    ///
    /// The returned value contains the complete positional policy recovered for
    /// ordinary production use. Callers supply only typed addresses; the HAL
    /// retains all register-facing profile choices. This value does not itself
    /// prove allocation, contents, lifetime, or exclusive ownership of either
    /// object.
    pub const fn normal_controller_profile(
        environment: DmaRef<BlePhyEnvironment>,
        resolving_list: DmaRef<BleResolvingList>,
    ) -> Self {
        Self {
            private_timing_source_byte: NORMAL_PROFILE_PRIVATE_TIMING_SOURCE_BYTE,
            environment,
            resolving_list,
            set_branch_control_0470_bit_18: NORMAL_PROFILE_SET_BRANCH_CONTROL_0470_BIT_18,
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

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Disable the BLE PHY ETM route for one Direct Test Mode event.
    ///
    /// SOURCE: pinned `libble_app.a[dtm_4.o]` event bodies
    /// `sym_dtm_2zeOUjc7g55zkDNQuZkg` and
    /// `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` disable, for a test without CTE,
    /// the vendor channel carrying the route that PHY register initialization
    /// installs; this owner carries that route on channel two.
    pub fn disable_ble_phy_etm_route(&mut self) -> BlePhyEtmRouteDisabled {
        self.etm.trigger_channel_enable_clear_ch2();
        BlePhyEtmRouteDisabled { _private: () }
    }

    /// Route and enable the BLE PHY ETM channel again after a test.
    ///
    /// SOURCE: pinned Test End `sym_dtm_NsbldBIeGraE2wg0AVy7` calls
    /// `r_ble_phy_init`, which leads to the PHY register initialization
    /// that routes and enables the channel.
    pub fn restore_ble_phy_etm_route(&mut self, route: BlePhyEtmRouteDisabled) {
        let BlePhyEtmRouteDisabled { _private: () } = route;
        route_and_enable(&mut self.etm);
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
    pub unsafe fn enable_ble_base_stack_hardware<B: Placement>(
        &mut self,
        baseband: &mut BtV32Baseband<B>,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        let mut transaction = HardwareBleBaseStackOnTaskEnableTransaction {
            registers: self,
            baseband,
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
    pub unsafe fn initialize_ble_phy_registers<B: Placement>(
        &mut self,
        baseband: &mut BtV32Baseband<B>,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        let timing_byte = inputs.private_timing_source_byte.wrapping_sub(1);

        let bluetooth = &mut self.bluetooth;
        let btmac = &mut bluetooth.btmac_ble_phy_init;

        // Ordered positional images and BTMAC prefix.
        bluetooth.ble_hw_positional_word_1ff0.write_value(|_| {});
        bluetooth.ble_hw_positional_word_1ff0.write_value(|w| {
            w.set_bit_0_opaque(true);
        });
        // Keep only source 17 of the interrupt enables: every other field of
        // the word is written zero.
        btmac.modify_interrupt_enable(|w| {
            w.set_scheduler_run(false);
            w.set_bits_0_13_opaque(oer_mmio::Bits::constant::<0>());
            w.set_bits_15_16_opaque(oer_mmio::Bits::constant::<0>());
            w.set_bits_18_31_opaque(oer_mmio::Bits::constant::<0>());
        });
        // Clear every source in one write of all ones.
        btmac.trigger_interrupt_clear(|t| {
            t.bits_0_13_opaque(oer_mmio::Bits::constant::<0x3fff>())
                .scheduler_run()
                .bits_15_31_opaque(oer_mmio::Bits::constant::<0x1_ffff>());
        });

        // `r_ble_phy_ramup_time_set`: the TX-on delay fields, then the
        // external-baseband LE TX-on delay it tail-calls.
        btmac.modify_lc_tx_on_delay_config(|w| {
            w.set_lc_tx_on_delay(0x0);
            w.set_init_duplicate_byte(0x0);
        });
        btmac.modify_lc_tx_on_delay_config(|w| {
            w.set_lc_tx_on_delay(w.lc_tx_on_delay() | timing_byte);
            w.set_init_duplicate_byte(w.init_duplicate_byte() | timing_byte);
        });
        // The LE TX-on delay field takes the byte less ten; the three bits
        // above it take the byte's bits 8..=10, zero for a byte.
        baseband.modify_le_tx_on_delay(|w| {
            w.set_encoded_value_minus_10(timing_byte.wrapping_sub(10));
            w.set_bits_16_18_opaque(oer_mmio::Bits::constant::<0>());
        });

        btmac.modify_word_0538_opaque(|w| {
            w.set_bits_0_31_opaque(0x65b);
        });
        btmac.modify_word_08a4_opaque(|w| {
            w.set_bits_0_7_opaque(0x2);
            w.set_bits_8_15_opaque(0x2);
            w.set_bits_16_23_opaque(0x2);
            w.set_bits_24_31_opaque(0x2);
        });
        btmac.modify_word_08a8_opaque(|w| {
            w.set_bits_0_7_opaque(0x2);
            w.set_bits_8_15_opaque(0x2);
            w.set_bits_16_23_opaque(0x2);
            w.set_bits_24_31_opaque(0x2);
        });
        btmac.modify_phy_environment_member_pointer(|w| {
            w.set_address(inputs.environment);
        });
        btmac.publish_ble_phy_init_value_04ac();
        btmac.modify_word_085c_opaque(|w| {
            w.set_bits_0_31_opaque(0x8);
        });

        // Four independent fresh-read updates at 0x20101654; the last two
        // are `r_ble_phy_monitor_bb_sync`, which then clears 0x20101474 and
        // publishes image 0x20 at 0x2010891c.
        btmac.modify_word_0654_opaque(|w| {
            w.set_init_byte_0(0x0);
            w.set_init_byte_1(0x0);
        });
        btmac.modify_word_0654_opaque(|w| {
            w.set_init_byte_0(w.init_byte_0() | 0x1);
            w.set_init_byte_1(w.init_byte_1() | 0x1);
        });
        btmac.modify_word_0654_opaque(|w| {
            w.set_init_byte_2_low_7(oer_mmio::Bits::constant::<0x0>());
        });
        btmac.modify_word_0654_opaque(|w| {
            w.set_init_byte_2_low_7(oer_mmio::Bits::truncate(w.init_byte_2_low_7().get() | 0x32));
            w.set_init_byte_2_high(true);
        });

        btmac.modify_word_0474_opaque(|w| {
            w.set_bits_0_31_opaque(0x0);
        });
        bluetooth.ble_hw_positional_word_891c.write_value(|w| {
            w.set_bits_5_6_opaque(oer_mmio::Bits::constant::<0x1>());
        });
        bluetooth.ble_hw_accelerator.modify_init_config(|w| {
            w.set_value(0x2f0);
        });
        bluetooth.ble_hw_accelerator.modify_init_sram_region_0(|w| {
            w.set_value(0x2f080000);
        });
        bluetooth.ble_hw_accelerator.modify_init_sram_region_1(|w| {
            w.set_value(0x2f000000);
        });
        bluetooth
            .ble_hw_resolving_list
            .write_base_pointer(inputs.resolving_list, |_| {});

        btmac.modify_word_0800_opaque(|w| {
            w.set_init_enable_31(true);
            w.set_init_enable_22(false);
            w.set_bit_21_opaque(false);
        });
        btmac.modify_word_0800_opaque(|w| {
            w.set_init_enable_22(true);
        });
        btmac.publish_ble_phy_init_configuration_word();

        // Each byte replacement is a distinct fresh-read RMW in vendor order.
        btmac.modify_word_0950_opaque(|w| {
            w.set_init_byte_0(0x3);
        });
        btmac.modify_word_0950_opaque(|w| {
            w.set_init_byte_1(0x3);
        });
        btmac.modify_word_0950_opaque(|w| {
            w.set_init_byte_2(0x44);
        });

        btmac.modify_word_0954_opaque(|w| {
            w.set_init_byte_0(0x10);
        });
        btmac.modify_word_0954_opaque(|w| {
            w.set_init_byte_1(0x10);
        });
        btmac.modify_word_0954_opaque(|w| {
            w.set_init_byte_2(0x3c);
        });
        btmac.modify_word_0954_opaque(|w| {
            w.set_init_byte_3(0x28);
        });

        btmac.modify_word_095c_opaque(|w| {
            w.set_init_byte_0(0x8);
        });
        btmac.modify_word_095c_opaque(|w| {
            w.set_init_byte_1(0x8);
        });
        btmac.modify_word_095c_opaque(|w| {
            w.set_init_byte_2(0x8);
        });
        btmac.modify_word_095c_opaque(|w| {
            w.set_init_byte_3(0x8);
        });

        btmac.modify_word_0958_opaque(|w| {
            w.set_init_byte_0(0xc);
        });
        btmac.modify_word_0958_opaque(|w| {
            w.set_init_byte_1(0x8);
        });
        btmac.modify_word_0958_opaque(|w| {
            w.set_init_byte_2(0xc);
        });
        btmac.modify_word_0958_opaque(|w| {
            w.set_init_byte_3(0xc);
        });

        btmac.modify_word_0858_opaque(|w| {
            w.set_init_high_half(0xf);
        });
        btmac.modify_word_094c_opaque(|w| {
            w.set_init_low_5(oer_mmio::Bits::constant::<0x12>());
        });

        if inputs.set_branch_control_0470_bit_18 {
            btmac.modify_device_table_control(|w| {
                w.set_bit_18_opaque(true);
            });
        }
        bluetooth.ble_hw_positional_word_891c.write_value(|w| {
            w.set_bits_5_6_opaque(oer_mmio::Bits::constant::<0x2>());
        });

        bluetooth
            .ble_hw_runtime_control
            .write_phy_init_configuration(|w| {
                w.set_value_low_8(RUNTIME_CONFIGURATION_LOW_BYTE);
                w.set_config_8(true);
            });
        bluetooth
            .ble_hw_runtime_control
            .write_phy_init_configuration_followup(|w| {
                w.set_value(0x1);
            });

        let controller = &mut bluetooth.bluetooth_controller_core;
        controller.write_word_0244_opaque(|w| {
            w.set_bits_0_31_opaque(0);
        });
        controller.write_gpio_diag_argument_1(|w| {
            w.set_value(0x55);
        });
        controller.write_gpio_diag_control(|w| {
            w.set_value(0xfff);
        });
        // The vendor routes and enables channel zero; this owner's channel
        // two carries the same route.
        route_and_enable(&mut self.etm);
        controller.write_phy_environment_tail_pointer(inputs.environment, |_| {});

        // One ordering boundary is a reviewed Rust-side addition. It does not
        // replace, merge, or reorder any vendor MMIO edge above.
        device_fence();
    }
}

#[cfg(test)]
mod tests;
