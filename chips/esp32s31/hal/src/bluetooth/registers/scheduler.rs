//! Restricted scheduler hardware-list publication transactions.

#![deny(unsafe_code)]

use oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage;

use crate::layouts::{BleSchedulerItem, BleSchedulerItemHardwareNext, DmaRef};

use crate::bluetooth::registers::{
    BluetoothPlacements, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothTaskRegisters, device_fence,
};

trait BluetoothSchedulerHardwareListHeadControl {
    fn order_descriptor_before_publication(&mut self);
    fn publish_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: Option<DmaRef<BleSchedulerItem>>,
    );
    fn order_after_publication(&mut self);
}

trait BluetoothSchedulerHardwareListHeadObservationControl {
    fn read_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead;
    fn order_after_observation(&mut self);
}

impl<P: BluetoothPlacements> BluetoothSchedulerHardwareListHeadObservationControl
    for BluetoothTaskRegisters<P>
{
    fn read_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        self.scheduler_hardware_list_head(index)
    }

    fn order_after_observation(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_hardware_list_head_observation(
    control: &mut impl BluetoothSchedulerHardwareListHeadObservationControl,
    index: BluetoothSchedulerHardwareListIndex,
) -> BluetoothSchedulerHardwareListHead {
    let head = control.read_head(index);
    control.order_after_observation();
    head
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothSchedulerHardwareListHeadRetirementDisposition {
    Empty,
    ExpectedHeadStillPublished,
    UnexpectedHeadChanged,
}

fn classify_scheduler_hardware_list_head_retirement(
    expected: BluetoothSchedulerHardwareListHead,
    observed: BluetoothSchedulerHardwareListHead,
) -> BluetoothSchedulerHardwareListHeadRetirementDisposition {
    if observed.address().is_none() {
        BluetoothSchedulerHardwareListHeadRetirementDisposition::Empty
    } else if observed == expected {
        BluetoothSchedulerHardwareListHeadRetirementDisposition::ExpectedHeadStillPublished
    } else {
        BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged
    }
}

impl<P: BluetoothPlacements> BluetoothSchedulerHardwareListHeadControl
    for BluetoothTaskRegisters<P>
{
    fn order_descriptor_before_publication(&mut self) {
        device_fence();
    }

    fn publish_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: Option<DmaRef<BleSchedulerItem>>,
    ) {
        let head = head
            .map_or(Some(HeadPointerImage::NULL), HeadPointerImage::new)
            .expect("a placed scheduler head is encodable");
        self.bluetooth
            .btdm_scheduler_table
            .modify_entry(index.register_index(), |w| {
                w.set_head_pointer(head);
            });
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_hardware_list_head_publication(
    control: &mut impl BluetoothSchedulerHardwareListHeadControl,
    index: BluetoothSchedulerHardwareListIndex,
    head: Option<DmaRef<BleSchedulerItem>>,
) -> BluetoothSchedulerHardwareListHeadPublished {
    control.order_descriptor_before_publication();
    control.publish_head(index, head);
    control.order_after_publication();
    let head = head.map_or(BluetoothSchedulerHardwareListHead::empty(), |head| {
        BluetoothSchedulerHardwareListHead::from_address(head)
            .expect("a placed scheduler head is encodable")
    });
    BluetoothSchedulerHardwareListHeadPublished { index, head }
}

trait BluetoothSchedulerRunEventControl {
    fn clear_scheduler_run_event_source(&mut self);
    fn enable_scheduler_run_event_source(&mut self);
    fn order_after_scheduler_run_event(&mut self);
}

impl<P: BluetoothPlacements> BluetoothSchedulerRunEventControl for BluetoothTaskRegisters<P> {
    fn clear_scheduler_run_event_source(&mut self) {
        self.bluetooth
            .btmac_ble_phy_init
            .trigger_interrupt_clear_scheduler_run();
    }

    fn enable_scheduler_run_event_source(&mut self) {
        self.bluetooth
            .btmac_ble_phy_init
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(true);
            });
    }

    fn order_after_scheduler_run_event(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_run_event_publication(control: &mut impl BluetoothSchedulerRunEventControl) {
    control.clear_scheduler_run_event_source();
    control.enable_scheduler_run_event_source();
    control.order_after_scheduler_run_event();
}

/// Head published to one scheduler hardware list: the compressed image of the
/// entry's head field, zero for an empty list.
///
/// A non-empty value proves only the controller pointer encoding. Allocation,
/// descriptor initialization, lifetime and exclusive hardware ownership stay
/// with the controller-memory layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerHardwareListHead(u32);

/// Why an SRAM address cannot represent a published scheduler-list item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerHardwareListHeadError {
    /// The address compresses to the reserved empty-head image.
    EncodesEmptyHead,
}

impl BluetoothSchedulerHardwareListHead {
    /// Construct the empty hardware-list head.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Construct a non-empty head from one validated scheduler-item address.
    pub const fn from_address(
        address: DmaRef<BleSchedulerItem>,
    ) -> Result<Self, BluetoothSchedulerHardwareListHeadError> {
        match HeadPointerImage::new(address) {
            None => Err(BluetoothSchedulerHardwareListHeadError::EncodesEmptyHead),
            Some(image) => Ok(Self(image.bits())),
        }
    }

    /// CPU address of the published scheduler item, or `None` for an empty
    /// list. The head field holds a scheduler-item link, decoded by the
    /// generated codec of that link.
    pub const fn address(self) -> Option<u32> {
        BleSchedulerItemHardwareNext::from_bits(self.0).address_address()
    }

    const fn from_image(image: oer_mmio::Bits<20>) -> Self {
        Self(image.get())
    }
}

/// One of the two positional scheduler command words whose START field is
/// cleared by insertion-end result paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerInsertionCommand {
    /// `SCHEDULER_COMMAND_0`, selected by insertion-begin/end result four.
    Zero,
    /// `SCHEDULER_COMMAND_1`, selected by insertion-begin/end result five.
    One,
}

/// Affine evidence that descriptor writes were ordered before one hardware-list
/// head publication and the trailing device fence completed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the published scheduler head must feed insertion-end ownership"]
pub struct BluetoothSchedulerHardwareListHeadPublished {
    index: BluetoothSchedulerHardwareListIndex,
    head: BluetoothSchedulerHardwareListHead,
}

impl BluetoothSchedulerHardwareListHeadPublished {
    /// Hardware-list index updated by the publication.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.index
    }

    /// Head retained by the published hardware-list entry.
    pub const fn head(&self) -> BluetoothSchedulerHardwareListHead {
        self.head
    }
}

/// Affine evidence that one insertion command no longer carries START and the
/// trailing device fence completed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the cleared command must feed insertion-end ownership"]
pub struct BluetoothSchedulerInsertionCommandStartCleared {
    command: BluetoothSchedulerInsertionCommand,
}

impl BluetoothSchedulerInsertionCommandStartCleared {
    /// Positional command whose START field was cleared.
    pub const fn command(&self) -> BluetoothSchedulerInsertionCommand {
        self.command
    }
}

/// Affine proof that every scheduler hardware-list head is empty after the
/// complete initialization transaction and its trailing device fence.
///
/// This does not prove that arbitrary software list containers are empty. The
/// Controller may use it only to seed a new source-owned exclusive list epoch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the cleared hardware-list epoch must be retained by scheduler ownership"]
pub struct BluetoothSchedulerHardwareListsCleared {
    _private: (),
}

impl BluetoothSchedulerHardwareListsCleared {
    /// Construct host-only initialization evidence without MMIO.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

/// Affine evidence that the synchronous scheduler-run subscriber completed.
///
/// The exact current subscriber acknowledges stale BTMAC source 14, enables
/// that source through a fresh-read field update and completes a trailing
/// device fence. The value also retains the matching head and dynamic
/// interrupt proof, so the final RUN command cannot be reached with a partial
/// or reordered prologue.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the complete scheduler-run event must feed the hardware RUN command"]
pub struct BluetoothSchedulerRunEventPublished {
    head: BluetoothSchedulerHardwareListHeadPublished,
    _interrupts: BluetoothSchedulerRunInterruptsPrepared,
}

impl BluetoothSchedulerRunEventPublished {
    /// Hardware list whose head remains retained through the run prologue.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }

    /// Exact head retained through the run prologue.
    pub const fn head(&self) -> BluetoothSchedulerHardwareListHead {
        self.head.head()
    }
}

/// Affine evidence that the hardware RUN command and trailing device fence
/// completed.
///
/// This value retains the complete typed run prologue: hardware-list head,
/// dynamic interrupt preparation and the synchronous BTMAC scheduler event.
/// It is not evidence of radio completion.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the hardware run command must feed controller ownership"]
pub struct BluetoothSchedulerHardwareRunCommandPublished {
    event: BluetoothSchedulerRunEventPublished,
}

/// Affine identity after one fresh fenced observation found its list empty.
///
/// The token consumes the complete RUN prologue and retains its exact list and
/// originally published head. It proves only that the hardware head was empty
/// at this post-completion observation; the software-list removal predicate
/// and descriptor reclamation remain separate.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the empty hardware-head observation must feed software-list removal"]
pub struct BluetoothSchedulerHardwareListHeadEmptyObserved {
    index: BluetoothSchedulerHardwareListIndex,
    completed_head: BluetoothSchedulerHardwareListHead,
}

impl BluetoothSchedulerHardwareListHeadEmptyObserved {
    /// Hardware list whose head was freshly observed empty.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.index
    }

    /// Originally published head retained by the completed RUN epoch.
    pub const fn completed_head(&self) -> BluetoothSchedulerHardwareListHead {
        self.completed_head
    }
}

/// Result of one bounded post-completion hardware-head observation.
#[must_use = "the retained RUN provenance must enter fail-stop handling or advance"]
pub enum BluetoothSchedulerHardwareListHeadRetirementObservation {
    /// The originally published head remains nonempty at the mandatory gate.
    /// The DTM sole-item owner must treat this as a fail-stop invariant.
    ExpectedHeadStillPublished {
        /// Unchanged affine RUN provenance retained for fail-stop handling.
        run: BluetoothSchedulerHardwareRunCommandPublished,
        /// Freshly observed typed head, retained only as diagnostic identity.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// A different nonempty head was observed after this RUN publication.
    UnexpectedHeadChanged {
        /// Unchanged affine RUN provenance; callers must fail closed.
        run: BluetoothSchedulerHardwareRunCommandPublished,
        /// Fresh conflicting head identity.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// The matching hardware-list head was freshly observed empty.
    EmptyObserved(BluetoothSchedulerHardwareListHeadEmptyObserved),
}

impl BluetoothSchedulerHardwareRunCommandPublished {
    /// Hardware list now admitted to scheduler execution.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.event.index()
    }

    /// Exact head retained while hardware owns scheduler execution.
    pub const fn head(&self) -> BluetoothSchedulerHardwareListHead {
        self.event.head()
    }
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Read a head without clearing a live or foreign publication.
    pub(super) fn scheduler_head_present(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> bool {
        execute_scheduler_hardware_list_head_observation(self, index)
            .address()
            .is_some()
    }

    /// Remove every published scheduler hardware-list head.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` member `btdm_sched.c`
    /// symbol `r_sym_bt_XPuqTHliEO5V9xpR7aJR`. Its first hardware transaction
    /// walks `0x2010_b000..=0x2010_b0f0` with stride `0x10`; every entry is
    /// freshly read, bits 19:0 are cleared, and bits 31:20 are preserved.
    ///
    /// This method deliberately does not expose the later software event and
    /// list initialization performed by the vendor function, and therefore
    /// does not claim that the complete controller or scheduler is running.
    pub fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        for index in 0..16 {
            let index = BluetoothSchedulerHardwareListIndex::new(index)
                .expect("the walk stays below sixteen lists")
                .register_index();
            self.bluetooth
                .btdm_scheduler_table
                .modify_entry(index, |w| {
                    w.set_head_pointer(
                        oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::NULL,
                    );
                });
        }
        device_fence();
        BluetoothSchedulerHardwareListsCleared { _private: () }
    }

    /// Read the currently published head of one scheduler hardware list.
    ///
    /// SOURCE: complete current insertion-begin
    /// `r_sym_bt_EabYtUaAIR05LXw3qZSA` and same-chip named
    /// `r_btdm_sched_get_hw_list_header`. Zero is returned as no head;
    /// nonzero images reconstruct a four-byte-aligned 0x2f-prefixed address.
    #[doc(hidden)]
    pub(crate) fn scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        let image = self
            .bluetooth
            .btdm_scheduler_table
            .read_entry(index.register_index())
            .head_pointer();
        BluetoothSchedulerHardwareListHead::from_image(image)
    }

    /// Read the current head of one hardware list once, then fence.
    ///
    /// SOURCE: complete current `r_sym_bt_qkNMymdnaJnYUfzpKgEp`, carried by
    /// `oer-vendor-lineage` from same-chip named
    /// `r_btdm_sched_delete_specified_items`, reads the head once after a set
    /// diagnostic BUSY sample and compares chained item start times against
    /// it before skipping them.
    pub fn observe_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        execute_scheduler_hardware_list_head_observation(self, index)
    }

    /// Observe whether the hardware list admitted by one RUN is now empty.
    ///
    /// SOURCE: the post-picker tail of current
    /// `brk_sym_sched_MzeSZzbQ4ZKhWW6Wu5jV`, carried by `oer-vendor-lineage`
    /// from same-chip named `r_sched_txn_onSchedHwListDone`, freshly reads the
    /// hardware-list head
    /// after the source manager becomes empty and asserts that head is empty
    /// before the following software-list removal call.
    ///
    /// This finite transaction consumes and returns the affine RUN provenance
    /// on both paths, so an old empty-head observation cannot be replayed
    /// against a later scheduler epoch. The trailing device fence orders the
    /// observation before any later CPU-side descriptor transition.
    pub fn observe_scheduler_hardware_list_head_retirement(
        &mut self,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerHardwareListHeadRetirementObservation {
        let index = run.index();
        let completed_head = run.head();
        let observed = execute_scheduler_hardware_list_head_observation(self, index);
        match classify_scheduler_hardware_list_head_retirement(completed_head, observed) {
            BluetoothSchedulerHardwareListHeadRetirementDisposition::Empty => {
                BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(
                    BluetoothSchedulerHardwareListHeadEmptyObserved {
                        index,
                        completed_head,
                    },
                )
            }
            BluetoothSchedulerHardwareListHeadRetirementDisposition::ExpectedHeadStillPublished => {
                BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                    run,
                    observed,
                }
            }
            BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged => {
                BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                    run,
                    observed,
                }
            }
        }
    }

    /// Order prior descriptor writes and publish one scheduler hardware-list
    /// head while preserving the unassigned upper twelve entry bits.
    ///
    /// SOURCE: complete current insertion-end
    /// `r_sym_bt_4KfpZh0Hu5NprlqcNu0D` and same-chip named
    /// `r_btdm_sched_set_hw_list_header`.
    ///
    /// # Safety
    ///
    /// The caller must own the scheduler insertion epoch, must have completed
    /// every CPU write to the addressed descriptor graph, must keep that graph
    /// alive until a later verified completion/removal transaction, and must
    /// serialize this publication with the scheduler interrupt owner. This
    /// method orders prior SRAM writes before the MMIO head update; it does not
    /// validate descriptor contents or establish completion ownership.
    ///
    /// A hardware-observed head is an outside identity, not a reference that
    /// may be published before the memory owner resolves it against its arena:
    ///
    /// ```compile_fail
    /// use oer_esp32s31_pac::{BluetoothTaskRegisters, BluetoothSchedulerHardwareListHead,
    ///     BluetoothSchedulerHardwareListIndex};
    /// fn republish(registers: &mut BluetoothTaskRegisters, observed: BluetoothSchedulerHardwareListHead) {
    ///     unsafe {
    ///         registers.publish_scheduler_hardware_list_head(
    ///             BluetoothSchedulerHardwareListIndex::ZERO, observed.address());
    ///     }
    /// }
    /// ```
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler-epoch prerequisites"
    )]
    pub unsafe fn publish_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: Option<DmaRef<BleSchedulerItem>>,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        execute_scheduler_hardware_list_head_publication(self, index, head)
    }

    /// Clear START in the positional command selected by insertion-end.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the matching insertion-begin result and
    /// must serialize the RMW with every command producer and interrupt owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the insertion result and command serialization"
    )]
    pub unsafe fn clear_scheduler_insertion_command_start(
        &mut self,
        command: BluetoothSchedulerInsertionCommand,
    ) -> BluetoothSchedulerInsertionCommandStartCleared {
        match command {
            BluetoothSchedulerInsertionCommand::Zero => {
                self.bluetooth
                    .bluetooth_controller_core
                    .modify_scheduler_command_0(|w| {
                        w.set_start(false);
                    });
            }
            BluetoothSchedulerInsertionCommand::One => {
                self.bluetooth
                    .bluetooth_controller_core
                    .modify_scheduler_command_1(|w| {
                        w.set_start(false);
                    });
            }
        }
        device_fence();
        BluetoothSchedulerInsertionCommandStartCleared { command }
    }

    /// Publish the synchronous base-stack scheduler event required before RUN.
    ///
    /// SOURCE: complete current `r_sym_bt_PVKilXLQPu1BjRkm4C6O` dispatches
    /// entry one of the linker-assembled scheduler broker table after dynamic
    /// interrupt preparation. The entry's only subscriber,
    /// `brk_sym_scan_b8ogEC3hTjGmqL5oRuyw`, acknowledges BTMAC source 14
    /// before enabling it through a fresh-read RMW. This typed transaction
    /// replaces the generic broker dispatch without making the synchronous
    /// hardware effect asynchronous.
    #[doc(hidden)]
    pub fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        execute_scheduler_run_event_publication(self);
        BluetoothSchedulerRunEventPublished {
            head,
            _interrupts: interrupts,
        }
    }

    /// Publish the finite scheduler hardware RUN command.
    ///
    /// Consuming [`BluetoothSchedulerRunEventPublished`] proves that head
    /// publication, dynamic interrupt preparation and the synchronous BTMAC
    /// subscriber all completed in the required order.
    #[doc(hidden)]
    pub fn publish_scheduler_hardware_run_command(
        &mut self,
        event: BluetoothSchedulerRunEventPublished,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        self.bluetooth
            .bluetooth_controller_core
            .write_scheduler_control(|writer| {
                writer.set_command(
                    oer_esp32s31_pac::bluetooth_controller_core::scheduler_control::Command::Run,
                );
            });
        device_fence();
        BluetoothSchedulerHardwareRunCommandPublished { event }
    }
}

#[cfg(test)]
mod tests;

pub(crate) mod cancellation;

pub(crate) mod insertion;

pub(crate) mod lock_modify;

pub(crate) mod runtime;

pub(crate) mod stop;
