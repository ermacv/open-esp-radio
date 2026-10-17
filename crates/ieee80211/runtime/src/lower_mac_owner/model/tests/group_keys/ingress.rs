use super::*;

#[derive(Clone, Copy)]
enum Change {
    Install,
    Replace,
    Select,
    Remove,
}

struct Ingress {
    air: Rc<ModelAir>,
    role: VifRole,
    stage: GroupRxStage,
    old: [u8; 16],
    new: [u8; 16],
    armed: bool,
    injected: bool,
    accepted: bool,
}

std::thread_local! {
    static INGRESS: RefCell<Option<Ingress>> = const { RefCell::new(None) };
}

struct IngressWake;
impl IngressWake {
    fn inject_once() {
        let injection = INGRESS.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.as_mut()?;
            if !state.armed {
                return None;
            }
            state.armed = false;
            state.injected = true;
            Some((
                state.air.clone(),
                state.role,
                state.stage,
                state.old,
                state.new,
            ))
        });
        if let Some((air, role, stage, old, new)) = injection {
            let accepted = air.receive_group(&[0xb], meta(), role, id(), &old, stage);
            // New-key traffic captured at the same wake belongs after the
            // terminal, even when it is already ready for delivery.
            air.receive_group(&[0xc], meta(), role, id(), &new, GroupRxStage::Ready);
            INGRESS.with(|state| state.borrow_mut().as_mut().unwrap().accepted = accepted);
        }
    }
}
impl std::task::Wake for IngressWake {
    fn wake(self: std::sync::Arc<Self>) {
        Self::inject_once();
    }
    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        Self::inject_once();
    }
}

struct Cut {
    role: VifRole,
    change: Change,
    stage: GroupRxStage,
}
impl Work for Cut {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let access_point_association = ModelKeys::<AccessPoint>::begin_association(
            resources.access_point_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let old = if matches!(self.change, Change::Select) {
            [2; 16]
        } else {
            [1; 16]
        };
        match self.role {
            VifRole::Station => {
                let [a_slot, b_slot] = resources.station_group;
                phase.station().install_group_key(
                    &station_association,
                    resources.station_group_permission,
                    a_slot,
                    id(),
                    Ccmp128Key::new(&[1; 16]),
                );
                let (a, mut permission) = installed(phase).await;
                let mut spare = Some(b_slot);
                let _retained = if matches!(self.change, Change::Select) {
                    phase.station().install_group_key(
                        &station_association,
                        permission,
                        spare.take().unwrap(),
                        id(),
                        Ccmp128Key::new(&[2; 16]),
                    );
                    let (b, returned) = installed(phase).await;
                    permission = returned;
                    Some(b)
                } else {
                    None
                };
                assert!(phase.held.air.receive_group(
                    &[0xa],
                    meta(),
                    self.role,
                    id(),
                    &old,
                    self.stage
                ));
                match self.change {
                    Change::Install => phase.station().install_group_key(
                        &station_association,
                        permission,
                        spare.unwrap(),
                        id(),
                        Ccmp128Key::new(&[2; 16]),
                    ),
                    Change::Replace => phase.station().replace_group_key(
                        permission,
                        a,
                        id(),
                        Ccmp128Key::new(&[2; 16]),
                    ),
                    Change::Select => phase.station().select_group_key(permission, a),
                    Change::Remove => phase.station().remove_group_key(permission, a),
                }
            }
            VifRole::AccessPoint => {
                let [a_slot, b_slot] = resources.access_point_group;
                phase.access_point().install_group_key(
                    &access_point_association,
                    resources.access_point_group_permission,
                    a_slot,
                    id(),
                    Ccmp128Key::new(&[1; 16]),
                );
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::AccessPoint { key, permission },
                )))) = phase.next_event().await
                else {
                    panic!("AP install")
                };
                assert!(phase.held.air.receive_group(
                    &[0xa],
                    meta(),
                    self.role,
                    id(),
                    &old,
                    self.stage
                ));
                match self.change {
                    Change::Install => phase.access_point().install_group_key(
                        &access_point_association,
                        permission,
                        b_slot,
                        id(),
                        Ccmp128Key::new(&[2; 16]),
                    ),
                    Change::Remove => phase.access_point().remove_group_key(permission, key),
                    Change::Replace | Change::Select => unreachable!(),
                }
            }
        }
        INGRESS.with(|state| state.borrow_mut().as_mut().unwrap().armed = true);
        let mut old_received = 0;
        loop {
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                    let bytes = frame.bytes().to_vec();
                    phase.recycle(frame);
                    assert_ne!(bytes, [0xc], "new-key RX preceded its terminal");
                    old_received += 1;
                }
                Ok(Ok(LowerMacEvent::Terminal(terminal))) => {
                    match (self.change, terminal) {
                        (Change::Install, LowerMacTerminal::GroupKeyInstalled(_))
                        | (Change::Replace, LowerMacTerminal::GroupKeyReplaced { .. })
                        | (Change::Select, LowerMacTerminal::GroupKeySelected { .. })
                        | (Change::Remove, LowerMacTerminal::KeyRemoved(_)) => {}
                        _ => panic!("wrong mutation terminal"),
                    }
                    break;
                }
                _ => panic!("unexpected boundary event"),
            }
        }
        assert!(
            old_received >= 1,
            "pre-cut DMA fact must precede the terminal"
        );
        let accepted = INGRESS.with(|state| state.borrow().as_ref().unwrap().accepted);
        assert_eq!(
            old_received,
            1 + usize::from(accepted),
            "accepted old ingress crossed its terminal"
        );
        let mut cx = core::task::Context::from_waker(Waker::noop());
        loop {
            let after = core::pin::pin!(phase.next_event()).as_mut().poll(&mut cx);
            match after {
                Poll::Ready(Ok(Ok(LowerMacEvent::Received { frame, .. }))) => {
                    let bytes = frame.bytes().to_vec();
                    phase.recycle(frame);
                    assert_ne!(bytes, [0xb], "old-key RX followed its terminal");
                }
                Poll::Pending => break,
                _ => panic!("unexpected post-cut event"),
            }
        }
    }
}

#[test]
fn ingress_at_a_delivery_wake_cannot_cross_a_group_key_terminal() {
    for (role, change) in [
        (VifRole::Station, Change::Install),
        (VifRole::Station, Change::Replace),
        (VifRole::Station, Change::Select),
        (VifRole::Station, Change::Remove),
        (VifRole::AccessPoint, Change::Install),
        (VifRole::AccessPoint, Change::Remove),
    ] {
        for stage in [
            GroupRxStage::Dma,
            GroupRxStage::Interrupt,
            GroupRxStage::Software,
        ] {
            let (backend, air) = backend();
            INGRESS.with(|state| {
                *state.borrow_mut() = Some(Ingress {
                    air: air.clone(),
                    role,
                    stage,
                    old: if matches!(change, Change::Select) {
                        [2; 16]
                    } else {
                        [1; 16]
                    },
                    new: if matches!(change, Change::Select) {
                        [1; 16]
                    } else {
                        [2; 16]
                    },
                    armed: false,
                    injected: false,
                    accepted: false,
                })
            });
            let waker = Waker::from(std::sync::Arc::new(IngressWake));
            let mut cx = core::task::Context::from_waker(&waker);
            let mut running = core::pin::pin!(run_port(
                backend,
                Body(Cut {
                    role,
                    change,
                    stage
                })
            ));
            let mut ended = false;
            for _ in 0..100 {
                if running.as_mut().poll(&mut cx).is_ready() {
                    ended = true;
                    break;
                }
            }
            let ingress = INGRESS.with(|state| state.borrow_mut().take().unwrap());
            assert!(
                ingress.injected,
                "ingress callback must execute during delivery"
            );
            assert!(
                ended,
                "boundary did not finish within a finite driver budget"
            );
            assert!(!air.failed());
        }
    }
}
