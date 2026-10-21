use super::*;
use oer_ieee80211_wire::{
    ccmp::{Ccmp128Key, CcmpPacketNumber, GroupKeyId},
    qos::WmmAccessCategory,
    security::Spp,
};

#[derive(Clone, Copy, Debug)]
enum Kind {
    Pairwise,
    PairwiseOtherHead,
    PairwiseRetryHead,
    GroupOtherHead,
    ConfigureOtherHead,
    ConfigureIndependent,
    DisableQueued,
    DisableIncoming,
}
impl Kind {
    fn disabling(self) -> bool {
        matches!(self, Self::DisableQueued | Self::DisableIncoming)
    }
    fn configuring(self) -> bool {
        matches!(self, Self::ConfigureOtherHead | Self::ConfigureIndependent)
    }
    fn cause(self) -> WithdrawCause {
        if self.disabling() || self.configuring() {
            WithdrawCause::Disabled
        } else {
            WithdrawCause::KeyRemoved
        }
    }
}

fn submit_ap<'k, 'p, 'id: 'p + 'k>(
    phase: &mut Esp32s31Phase<'p, 'id, TestDevice, 512>,
    buffer: Buffer<'id, 512>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<oer_radio_port::LocalId<'id>>, 2>>,
    key: Option<&'k GroupKey<'id, AccessPoint>>,
) {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("rate");
    let frame = ClassFrame::new(TxFrame::Group {
        frame: SizedFrame::new(
            HeaderSpec::new(
                FrameControl::new(if key.is_some() { 0x4208 } else { 0x0208 })
                    .expect("group control"),
                0,
                GroupAddress::new([0xff; 6]).expect("group receiver"),
                ACCESS_POINT,
                BSSID,
                0,
                None,
                None,
                None,
            )
            .expect("AP header"),
            MpduLength::new(64).expect("length"),
        )
        .expect("group frame"),
        rate,
    })
    .ok()
    .expect("class");
    let source = TxSource::Fresh {
        buffer,
        frame,
        body: None,
        protection: match key {
            Some(key) => TxProtection::Ccmp(NumberedKey {
                key: AttemptKey::Group(key),
                packet_number: CcmpPacketNumber::new(1).expect("PN"),
            }),
            None => TxProtection::Open,
        },
    }
    .check()
    .ok()
    .expect("AP layout");
    let source =
        <Family as TxBufferPool<'id, BufferClass<512>>>::prepare::<AccessPoint, Caps>(source)
            .ok()
            .expect("AP preparation");
    phase
        .access_point()
        .submit(
            Attempt {
                context: 92,
                ticket,
                source,
                access_category: WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                coex_operation: TxOperation::Data,
            }
            .admit(),
        )
        .expect("AP submit");
}

struct Withdraw {
    kind: Kind,
    observed: Rc<RefCell<Observed>>,
}
type Row = (u8, bool, SingleOutcome<Ack, Infallible>);
fn record<'id>(
    completed: TxCompleted<'id, Family>,
    access: &mut TerminalAccess<
        'id,
        Tickets,
        radio::Child<radio::Root<oer_radio_port::LocalId<'id>>, 2>,
    >,
) -> Row {
    let TxCompleted::Single(completed) = completed else {
        panic!("single terminal")
    };
    let parts = completed.into_parts(access);
    let (buffer, _) = parts.payload.into_parts();
    (
        parts.ticket.index(),
        matches!(buffer, AnyBuffer::Sent(_)),
        parts.outcome,
    )
}

impl PortBody<Backend> for Withdraw {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        mut resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        let rows = Rc::new(RefCell::new(Vec::<Row>::new()));
        let [head, tail, last, ..] = resources.buffers;
        let [head_ticket, tail_ticket, last_ticket, ..] = resources.tickets;
        let expected = [
            head_ticket.index(),
            tail_ticket.index(),
            last_ticket.index(),
        ];
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let ([slot], _) = bank.into_parts();
            let station = start.begin(Spp::NotNegotiated);
            let key = phase.station().install_pairwise_key(
                &station,
                slot,
                IndividualAddress::new(BSSID).expect("peer"),
                Ccmp128Key::new(&[7; 16]),
            );
            let (start, bank) = resources.access_point_association.into_parts();
            let (_, [slot, ..]) = bank.into_parts();
            let ap = start.begin(Spp::NotNegotiated);
            phase.access_point().install_group_key(
                &ap,
                resources.access_point_group_permission,
                slot,
                GroupKeyId::new(1).expect("KeyID"),
                Ccmp128Key::new(&[8; 16]),
            );
            let (group, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::AccessPoint { key, permission },
                )))) => (key, permission),
                _ => panic!("AP GTK installed"),
            };
            self.observed.borrow_mut().hold_completion = true;
            submit_mpdu_budget(
                &mut phase,
                head,
                head_ticket,
                None,
                matches!(self.kind, Kind::Pairwise).then_some(&key),
                if matches!(self.kind, Kind::PairwiseRetryHead) {
                    2
                } else {
                    1
                },
            );
            for _ in 0..4 {
                yield_once().await;
            }
            assert_eq!(self.observed.borrow().publications, 1);
            if !matches!(self.kind, Kind::ConfigureIndependent) {
                for (buffer, ticket) in [(tail, tail_ticket), (last, last_ticket)] {
                    match self.kind {
                        Kind::GroupOtherHead => submit_ap(&mut phase, buffer, ticket, Some(&group)),
                        Kind::ConfigureOtherHead => submit_ap(&mut phase, buffer, ticket, None),
                        _ => submit_mpdu(
                            &mut phase,
                            buffer,
                            ticket,
                            None,
                            (!self.kind.disabling()).then_some(&key),
                        ),
                    }
                }
            }
            match self.kind {
                Kind::Pairwise | Kind::PairwiseOtherHead | Kind::PairwiseRetryHead => {
                    phase.station().remove_pairwise_key(station, key)
                }
                Kind::GroupOtherHead => phase.access_point().remove_group_key(permission, group),
                Kind::ConfigureOtherHead | Kind::ConfigureIndependent => {
                    phase.access_point().configure(
                        resources.access_point_commands.configure.with_context(314),
                        SupportedFilter::new(ReceiveFilter::NONE).expect("filter"),
                    )
                }
                _ => {}
            }
            if !matches!(self.kind, Kind::DisableIncoming) {
                for _ in 0..4 {
                    yield_once().await;
                }
            }
            if matches!(self.kind, Kind::ConfigureIndependent) {
                assert!(matches!(
                    phase.next_event().await,
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                        Configured::AccessPoint(_)
                    ))))
                ));
            } else if !self.kind.disabling() {
                let mut event = pin!(phase.next_event());
                assert!(
                    core::future::poll_fn(|cx| Poll::Ready(event.as_mut().poll(cx)))
                        .await
                        .is_pending(),
                    "the tail cannot overtake a published head on the same AC"
                );
            }
            if !self.kind.disabling() {
                self.observed.borrow_mut().hold_completion = false;
                let count = if matches!(self.kind, Kind::ConfigureIndependent) {
                    1
                } else {
                    3
                };
                for _ in 0..count {
                    let completed = match phase.next_event().await {
                        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                            completed,
                        )))) => completed,
                        _ => panic!("ordered TX terminal"),
                    };
                    rows.borrow_mut()
                        .push(record(completed, &mut resources.terminal_access));
                }
                if !matches!(self.kind, Kind::ConfigureIndependent) {
                    match phase.next_event().await {
                        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_))))
                            if !self.kind.configuring() => {}
                        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                            Configured::AccessPoint(permission),
                        )))) if self.kind.configuring() => {
                            assert_eq!(permission.into_parts().1, 314)
                        }
                        _ => panic!("barrier after TX owners"),
                    }
                }
            }
        }
        let owner = if self.kind.disabling() {
            let captured = rows.clone();
            let mut sink = |terminal| {
                let LowerMacTerminal::TxCompleted(completed) = terminal else {
                    panic!("TX drain terminal")
                };
                captured
                    .borrow_mut()
                    .push(record(completed, &mut resources.terminal_access));
                core::ops::ControlFlow::Continue(())
            };
            let mut disable = pin!(owner.disable(&mut sink));
            for _ in 0..4 {
                assert!(
                    core::future::poll_fn(|cx| Poll::Ready(disable.as_mut().poll(cx)))
                        .await
                        .is_pending()
                );
                assert!(
                    rows.borrow().is_empty(),
                    "disable retains tails behind the published head"
                );
                yield_once().await;
            }
            self.observed.borrow_mut().hold_completion = false;
            match disable.await {
                Ok(owner) => owner,
                Err(_) => panic!("disable"),
            }
        } else {
            match owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
            {
                Ok(owner) => owner,
                Err(_) => panic!("disable"),
            }
        };
        let rows = rows.borrow();
        let count = if matches!(self.kind, Kind::ConfigureIndependent) {
            1
        } else {
            3
        };
        assert_eq!(rows.len(), count);
        for (position, (ticket, sent, outcome)) in rows.iter().enumerate() {
            assert_eq!(*ticket, expected[position], "original AC order");
            assert_eq!(*sent, position == 0);
            if position == 0 {
                assert!(outcome.is_delivered());
            } else {
                assert_eq!(*outcome, TxOutcome::Withdrawn(self.kind.cause()));
            }
        }
        assert_eq!(
            self.observed.borrow().publications,
            if matches!(self.kind, Kind::PairwiseRetryHead) {
                2
            } else {
                1
            }
        );
        radio::Ended::uninstall(owner, (), ())
    }
}

fn check(kind: Kind) {
    let (backend, observed, _) = installed(if matches!(kind, Kind::PairwiseRetryHead) {
        std::vec![(5, 0), (0, 0)]
    } else {
        Vec::new()
    });
    assert!(matches!(
        block_on(radio::run_port(
            backend,
            Withdraw { kind, observed },
            oer_radio_port::LocalExecution
        )),
        radio::PortEnd::Uninstalled { .. }
    ));
}
#[test]
fn pairwise_remove_keeps_its_published_head_before_tails() {
    check(Kind::Pairwise)
}
#[test]
fn pairwise_remove_keeps_an_unrelated_published_head_before_tails() {
    check(Kind::PairwiseOtherHead)
}
#[test]
fn selective_withdrawal_keeps_an_unaffected_sent_retry_before_tails() {
    check(Kind::PairwiseRetryHead)
}
#[test]
fn group_remove_keeps_an_unrelated_published_head_before_tails() {
    check(Kind::GroupOtherHead)
}
#[test]
fn configure_keeps_its_tails_behind_the_other_vifs_published_head() {
    check(Kind::ConfigureOtherHead)
}
#[test]
fn configure_without_its_own_tail_does_not_wait_for_the_other_vif() {
    check(Kind::ConfigureIndependent)
}
#[test]
fn disable_keeps_already_queued_tails_behind_the_published_head() {
    check(Kind::DisableQueued)
}
#[test]
fn stopping_submit_keeps_incoming_tails_behind_the_published_head() {
    check(Kind::DisableIncoming)
}
