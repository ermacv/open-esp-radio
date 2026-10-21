//! Select typed lower-MAC views at each synchronous P2 frontier.
use super::*;

#[inline]
pub(super) fn configuring<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
) -> bool {
    match view {
        CoreView::ConfigureDisabled(_) => true,
        CoreView::ConfigureDisabledClosing(_) => true,
        CoreView::ConfigureDisabledClosed(_) => true,
        CoreView::ConfigureOpen(_) => true,
        CoreView::ConfigureClosing(_) => true,
        CoreView::ConfigureClosed(_) => true,
        _ => false,
    }
}

#[inline]
pub(super) fn deadline<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
) -> Option<Instant> {
    match view {
        CoreView::Disabled(view) => view.next_deadline(),
        CoreView::DisabledClosing(view) => view.next_deadline(),
        CoreView::DisabledClosed(view) => view.next_deadline(),
        CoreView::Retuning(view) => view.next_deadline(),
        CoreView::RetuningClosing(view) => view.next_deadline(),
        CoreView::RetuningClosed(view) => view.next_deadline(),
        CoreView::Open(view) => view.next_deadline(),
        CoreView::Closing(view) => view.next_deadline(),
        CoreView::Closed(view) => view.next_deadline(),
        CoreView::Disabling(view) => view.next_deadline(),
        CoreView::DisablingClosing(view) => view.next_deadline(),
        CoreView::DisablingClosed(view) => view.next_deadline(),
        CoreView::ConfigureDisabled(view) => view.next_deadline(),
        CoreView::ConfigureDisabledClosing(view) => view.next_deadline(),
        CoreView::ConfigureDisabledClosed(view) => view.next_deadline(),
        CoreView::ConfigureOpen(view) => view.next_deadline(),
        CoreView::ConfigureClosing(view) => view.next_deadline(),
        CoreView::ConfigureClosed(view) => view.next_deadline(),
        CoreView::Failed(view) => view.next_deadline(),
        _ => None,
    }
}

#[inline]
pub(super) fn reset_contention<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    role: VifRole,
    category: WmmAccessCategory,
) {
    match view {
        CoreView::Disabled(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::DisabledClosing(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::DisabledClosed(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::Retuning(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::RetuningClosing(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::RetuningClosed(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::Open(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::Closing(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::Closed(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::Disabling(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::DisablingClosing(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::DisablingClosed(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::ConfigureDisabled(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::ConfigureDisabledClosing(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::ConfigureDisabledClosed(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::ConfigureOpen(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::ConfigureClosing(mut view) => {
            view.reset_contention(role, category);
        }
        CoreView::ConfigureClosed(mut view) => {
            view.reset_contention(role, category);
        }
        _ => {}
    }
}

#[inline]
pub(super) fn service<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    hardware: &mut D::Hardware,
    wake: WifiTxWake,
    sink: &mut Sink<'id, Id, B>,
) -> Result<(), LowerMacFault> {
    match view {
        CoreView::Disabled(mut view) => view.service(hardware, wake, sink),
        CoreView::DisabledClosing(mut view) => view.service(hardware, wake, sink),
        CoreView::DisabledClosed(mut view) => view.service(hardware, wake, sink),
        CoreView::Retuning(mut view) => view.service(hardware, wake, sink),
        CoreView::RetuningClosing(mut view) => view.service(hardware, wake, sink),
        CoreView::RetuningClosed(mut view) => view.service(hardware, wake, sink),
        CoreView::Open(mut view) => view.service(hardware, wake, sink),
        CoreView::Closing(mut view) => view.service(hardware, wake, sink),
        CoreView::Closed(mut view) => view.service(hardware, wake, sink),
        CoreView::Disabling(mut view) => view.service(hardware, wake, sink),
        CoreView::DisablingClosing(mut view) => view.service(hardware, wake, sink),
        CoreView::DisablingClosed(mut view) => view.service(hardware, wake, sink),
        CoreView::ConfigureDisabled(mut view) => view.service(hardware, wake, sink),
        CoreView::ConfigureDisabledClosing(mut view) => view.service(hardware, wake, sink),
        CoreView::ConfigureDisabledClosed(mut view) => view.service(hardware, wake, sink),
        CoreView::ConfigureOpen(mut view) => view.service(hardware, wake, sink),
        CoreView::ConfigureClosing(mut view) => view.service(hardware, wake, sink),
        CoreView::ConfigureClosed(mut view) => view.service(hardware, wake, sink),
        CoreView::Failed(mut view) => view.service(hardware, wake, sink),
        _ => Ok(()),
    }
}

#[inline]
pub(super) fn received<'frame, 'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    hardware: &D::Hardware,
    frame: &NormalizedRxFrame<'frame>,
) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
    match view {
        CoreView::Open(view) => view.received(hardware, frame),
        CoreView::Closing(view) => view.received(hardware, frame),
        CoreView::Closed(view) => view.received(hardware, frame),
        CoreView::ConfigureOpen(view) => view.received(hardware, frame),
        CoreView::ConfigureClosing(view) => view.received(hardware, frame),
        CoreView::ConfigureClosed(view) => view.received(hardware, frame),
        _ => None,
    }
}

#[inline]
pub(super) fn tune<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    channel: oer_ieee80211_wire::channel::WifiChannel,
    power: Option<D::Power>,
) -> Result<(), Fault> {
    match view {
        CoreView::Disabled(mut view) => {
            if let Some(power) = power {
                view.replace_power_profile(power);
            }
            view.set_checked_channel(channel);
            view.radio_tuned_to(channel);
            match view.enable_checked() {
                LifecycleStart::Enabled(_) => Ok(()),
                LifecycleStart::Retune(_) => {
                    panic!("radio start recorded the requested channel")
                }
            }
        }
        CoreView::DisabledClosing(mut view) => {
            if let Some(power) = power {
                view.replace_power_profile(power);
            }
            view.set_checked_channel(channel);
            view.radio_tuned_to(channel);
            match view.enable_checked() {
                LifecycleStart::Enabled(_) => Ok(()),
                LifecycleStart::Retune(_) => {
                    panic!("radio start recorded the requested channel")
                }
            }
        }
        CoreView::DisabledClosed(mut view) => {
            if let Some(power) = power {
                view.replace_power_profile(power);
            }
            view.set_checked_channel(channel);
            view.radio_tuned_to(channel);
            match view.enable_checked() {
                LifecycleStart::Enabled(_) => Ok(()),
                LifecycleStart::Retune(_) => {
                    panic!("radio start recorded the requested channel")
                }
            }
        }
        _ => Err(Fault::Capacity),
    }
}

#[inline]
pub(super) fn configure<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    config: CoreVif,
) -> Result<(), Fault> {
    match view {
        CoreView::Open(view) => view
            .configure(config)
            .map(|_| ())
            .map_err(|_| Fault::Capacity),
        CoreView::Closing(view) => view
            .configure(config)
            .map(|_| ())
            .map_err(|_| Fault::Capacity),
        CoreView::Closed(view) => view
            .configure(config)
            .map(|_| ())
            .map_err(|_| Fault::Capacity),
        _ => Err(Fault::Capacity),
    }
}

#[inline]
pub(super) fn advance_configuration<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    hardware: &mut D::Hardware,
    sink: &mut Sink<'id, Id, B>,
) -> bool {
    match view {
        CoreView::ConfigureDisabled(view) => {
            matches!(
                view.advance(hardware, sink),
                ConfigurationProgress::Configured(_)
            )
        }
        CoreView::ConfigureDisabledClosing(view) => {
            matches!(
                view.advance(hardware, sink),
                ConfigurationProgress::Configured(_)
            )
        }
        CoreView::ConfigureDisabledClosed(view) => {
            matches!(
                view.advance(hardware, sink),
                ConfigurationProgress::Configured(_)
            )
        }
        CoreView::ConfigureOpen(view) => {
            matches!(
                view.advance(hardware, sink),
                ConfigurationProgress::Configured(_)
            )
        }
        CoreView::ConfigureClosing(view) => {
            matches!(
                view.advance(hardware, sink),
                ConfigurationProgress::Configured(_)
            )
        }
        CoreView::ConfigureClosed(view) => {
            matches!(
                view.advance(hardware, sink),
                ConfigurationProgress::Configured(_)
            )
        }
        _ => true,
    }
}

#[inline]
pub(super) fn hold<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
) {
    match view {
        CoreView::Open(view) => {
            let _closing = view.hold_new_attempts();
        }
        _ => {}
    }
}

#[inline]
pub(super) fn close<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    hardware: &mut D::Hardware,
) -> bool {
    match view {
        CoreView::Closing(view) => {
            matches!(view.close_gate(hardware), GateClosure::Closed(_))
        }
        CoreView::Closed(_) => true,
        _ => false,
    }
}

#[inline]
pub(super) fn open<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    hardware: &mut D::Hardware,
    sink: &mut Sink<'id, Id, B>,
) -> Result<(), Fault> {
    match view {
        CoreView::Closed(view) => view
            .open_gate(hardware, sink)
            .map(|_| ())
            .map_err(|failed| Fault::Core(failed.fault())),
        _ => Err(Fault::Capacity),
    }
}

#[inline]
pub(super) fn disable<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    hardware: &mut D::Hardware,
    sink: &mut Sink<'id, Id, B>,
) {
    match view {
        CoreView::Open(view) => {
            let _disabling = view.disable_checked(hardware, sink);
        }
        CoreView::Closing(view) => {
            let _disabling = view.disable_checked(hardware, sink);
        }
        CoreView::Closed(view) => {
            let _disabling = view.disable_checked(hardware, sink);
        }
        _ => {}
    }
}

#[inline]
pub(super) fn finish_disable<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    view: CoreView<'_, Core<'id, Id, D, B>>,
    sink: &mut Sink<'id, Id, B>,
) -> bool {
    match view {
        CoreView::Disabling(view) => view.finish(sink).is_ok(),
        CoreView::DisablingClosing(view) => view.finish(sink).is_ok(),
        CoreView::DisablingClosed(view) => view.finish(sink).is_ok(),
        CoreView::Disabled(_) => true,
        CoreView::DisabledClosing(_) => true,
        CoreView::DisabledClosed(_) => true,
        _ => false,
    }
}
