use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_protocol::lower_mac::*;
use oer_hil_scenario::{
    Result,
    workload::{context::Context, require_keys},
};
use oer_ieee80211_port::{
    Band, Channel, ChannelWidth,
    backend_protection::BasicRates,
    conformance::{
        Execution,
        catalog::{Case, Extension},
        outcomes,
    },
};
use oer_ieee80211_upper_mac::rate_selection::management_rate;
use oer_lab_router::{router, ssh::Ssh};
use sha2::{Digest as _, Sha256};
use std::{
    future::Future,
    path::Path,
    rc::Rc,
    task::{Context as TaskContext, Poll, Waker},
    time::Duration,
};

pub fn run(output: &Path, context: &Context<'_>, selected: Option<Case>) -> Result<()> {
    context.results.claim(
        if selected.is_some() {
            "lower-mac-ownership-close-gate"
        } else {
            "lower-mac-conformance"
        },
        &[],
    );
    let StationFixtureConfig::OpenWrt(config) = &context.lab.station_fixture else {
        return Err("managed OpenWrt AP required".into());
    };
    let directory = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router = oer_lab_router::lock::router(&directory, &config.ssh_target)?;
    let mut ssh = Ssh::new(&config.ssh_target)?;
    let ssid = oer_lab_router::credentials::ssid("oer-lower-mac")?;
    let profile = router::Profile::new(
        &config.radio,
        &config.ap_section,
        &config.wireless_interface,
        config.channel,
        ssid.expose(),
    )?;
    let applied = router::apply(&mut ssh, &profile)?;
    context.with_capture(output, |capture| {
        require_keys::<Prepare>(capture)?;
        let prepared = capture.request(0, Prepare {channel: applied.channel, country: applied.country,
            peer_sha256: Sha256::digest(applied.bssid.octets()).into()}, Duration::from_secs(45))?;
        let mut passed = 0;
        let mut model_only = 0;
        let unsupported: Vec<_> = Case::all().enumerate().filter(|(_, case)| {
            selected.is_none_or(|wanted| wanted == *case) && case.execution() == Execution::Hardware
                && case.extension() == Some(Extension::Ampdu) && !prepared.ampdu
        }).map(|(index, case)| {
            let reason = format!("{case:?}: не поддерживается бэкендом: нет AmpduPhase; до W6");
            context.results.observe(format!("gate-e-skipped-{index:02}"),
                &serde_json::json!({"status":"skipped","case":index,"reason":"backend has no AmpduPhase","until":"W6"}));
            eprintln!("гейт e: пропущено: {reason}");
            reason
        }).collect();
        for (index,case) in Case::all().enumerate() {
            if selected.is_some_and(|wanted|wanted != case) {continue;}
            if let Execution::ModelOnly(_) = case.execution() {model_only += 1; continue}
            if case.extension() == Some(Extension::Ampdu) && !prepared.ampdu {
                continue;
            }
            let key = RunCase(u8::try_from(index)?);
            let hardware = capture.request(0,key,Duration::from_secs(15))?;
            context.results.observe(format!("case-{index:02}-hardware"), &hardware);
            if hardware.case != key {return Err("lower-MAC returned a different case identity".into())}
            let model = model(case, &prepared, applied.channel)?;
            context.results.observe(format!("case-{index:02}-model"), &model);
            if !traces_match(&hardware.trace, &model) {
                context.results.observe(if selected.is_some() {"diagnostic-result"} else {"gate-e"}, &serde_json::json!({"pass":false,"case":index,"reason":"trace mismatch"}));
                return Err(format!("lower-MAC trace differs from ModelBackend in {case:?}; stop for owner/architect decision").into());
            }
            passed += 1;
        }
        context.results.observe(if selected.is_some() {"diagnostic-result"} else {"gate-e"}, &serde_json::json!({"pass":true,"passed":passed,"model_only":model_only,
            "unsupported":unsupported}));
        eprintln!("lower-MAC gate e: {passed} hardware traces equal ModelBackend; {model_only} model-only");
        Ok(())
    })
}
fn traces_match(left: &Trace, right: &Trace) -> bool {
    let (Trace::Ownership(left), Trace::Ownership(right)) = (left, right) else {
        return left == right;
    };
    if left.returned != right.returned
        || left.contexts_dropped != right.contexts_dropped
        || left.before_teardown_dropped != right.before_teardown_dropped
        || left.publications != right.publications
        || !left
            .publication_events
            .iter()
            .map(Option::is_some)
            .eq(right.publication_events.iter().map(Option::is_some))
    {
        return false;
    }
    use oer_ieee80211_port::conformance::{PublicationEvent, same_publications_by_ac};
    let publication = |event: &OwnerPublication| match event {
        OwnerPublication::Tx(ticket) => PublicationEvent::Tx(*ticket),
        OwnerPublication::GateClosed => PublicationEvent::GateClosed,
    };
    same_publications_by_ac(
        left.publication_events.iter().flatten().map(publication),
        right.publication_events.iter().flatten().map(publication),
        |ticket| {
            left.returned
                .iter()
                .flatten()
                .find(|row| row.actual.ticket == ticket)
                .and_then(|row| oer_ieee80211_wire::qos::WmmAccessCategory::from_aci(row.category))
        },
    )
}

fn model(case: Case, prepared: &Prepared, channel: u8) -> Result<Trace> {
    let rates = prepared
        .basic_rates
        .get(..usize::from(prepared.basic_rates_len))
        .ok_or("invalid basic-rate extent")?;
    let basic = BasicRates::from_rate_elements(rates, &[]);
    let rate = management_rate(Band::Ghz2_4, Some(&basic)).ok_or("no basic management rate")?;
    let channel = Channel::ghz2_4(channel, ChannelWidth::Mhz20).map_err(|_| "invalid channel")?;
    if let Case::Edca(case) = case {
        let raw = complete(oer_ieee80211_model::conformance::edca(
            case,
            channel,
            rate,
            Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        ))??;
        let trace = edca_trace(&raw).ok_or("unrepresentable model EDCA trace")?;
        oer_ieee80211_port::conformance::edca::verify(case, raw)
            .map_err(|why| format!("model EDCA conformance violation: {why:?}"))?;
        return Ok(Trace::Edca(trace));
    }
    if let Case::Group(case) = case {
        use oer_ieee80211_port::LowerMacBackend;
        let raw = complete(oer_ieee80211_model::conformance::group(
            case,
            channel,
            rate,
            Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        ))??;
        let bound = oer_ieee80211_model::ModelBackend::<oer_ieee80211_model::ModelCaps>::GROUP_COMMAND_BOUND;
        let trace =
            group_trace(&raw, bound.as_micros()).ok_or("unrepresentable model group progress")?;
        oer_ieee80211_port::conformance::group_keys::verify_progress(case, raw, bound)
            .map_err(|why| format!("model group conformance violation: {why:?}"))?;
        return Ok(Trace::Group(trace));
    }
    if let Case::Configure(case) = case {
        let raw = complete(oer_ieee80211_model::conformance::configure(
            case,
            channel,
            Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        ))??;
        let trace = configure_trace(&raw).ok_or("unrepresentable model configuration")?;
        oer_ieee80211_port::conformance::interface_commands::verify(raw)
            .map_err(|why| format!("model configuration conformance violation: {why:?}"))?;
        return Ok(Trace::Configure(trace));
    }
    if let Case::Barrier(case) = case {
        let raw = complete(oer_ieee80211_model::conformance::barrier(
            case,
            channel,
            rate,
            Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        ))??;
        let trace = barrier_trace(&raw).ok_or("unrepresentable model barrier")?;
        oer_ieee80211_port::conformance::verify_barrier(case, raw)
            .map_err(|why| format!("model barrier conformance violation: {why:?}"))?;
        return Ok(Trace::Barrier(trace));
    }
    if let Case::Ownership(scenario) = case {
        let raw = complete(oer_ieee80211_model::conformance::ownership(
            scenario,
            channel,
            rate,
            Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        ))??;
        oer_ieee80211_port::conformance::verify(scenario, &raw)
            .map_err(|why| format!("model ownership conformance violation: {why:?}"))?;
        let trace = ownership_trace(raw).ok_or("unrepresentable model ownership")?;
        return Ok(Trace::Ownership(trace));
    }
    let Case::Outcome(case) = case else {
        return Err("model fixture for this suite is not available".into());
    };
    let raw = complete(oer_ieee80211_model::conformance::outcome(
        case,
        channel,
        rate,
        Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
    ))??;
    let trace = outcome_trace(&raw).ok_or("unrepresentable model outcome")?;
    outcomes::verify(case, raw).map_err(|why| format!("model conformance violation: {why:?}"))?;
    Ok(Trace::Outcome(trace))
}
fn complete<F: Future>(future: F) -> Result<F::Output> {
    let mut future = core::pin::pin!(future);
    let mut context = TaskContext::from_waker(Waker::noop());
    for _ in 0..10000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return Ok(value);
        }
    }
    Err("model made no bounded progress at the owner's virtual time".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_comparison_projects_publications_and_keeps_terminals_and_gate_exact() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            basic_rates_len: 1,
        };
        let expected = model(
            Case::Ownership(oer_ieee80211_port::conformance::Scenario::CloseGate),
            &prepared,
            6,
        )
        .expect("model ownership");
        let mut actual = expected.clone();
        if let Trace::Ownership(trace) = &mut actual {
            trace.publication_events.swap(1, 2); // 2,3,2,3 instead of 2,2,3,3.
        }
        assert_ne!(actual, expected);
        assert!(traces_match(&actual, &expected));
        let mut wrong_terminal = actual.clone();
        if let Trace::Ownership(trace) = &mut wrong_terminal {
            trace.returned[0]
                .as_mut()
                .expect("VO terminal")
                .transmissions = Some(1);
        }
        assert!(!traces_match(&wrong_terminal, &expected));
        if let Trace::Ownership(trace) = &mut actual {
            trace.publication_events.swap(3, 4);
        }
        assert!(!traces_match(&actual, &expected));
    }

    #[test]
    fn every_physical_edca_case_preserves_actual_draws_tables_and_fifo() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c; 16],
            basic_rates_len: 1,
        };
        let mut count = 0;
        for case in oer_ieee80211_port::conformance::edca::Case::ALL {
            if case.execution() != Execution::Hardware {
                continue;
            }
            let Trace::Edca(trace) = model(Case::Edca(case), &prepared, 6).expect("model EDCA")
            else {
                panic!("EDCA trace")
            };
            assert_eq!(trace.draws.iter().flatten().count(), 3);
            assert_eq!(trace.admitted, trace.completed);
            assert!(trace.installed.iter().flatten().all(Option::is_some));
            count += 1;
        }
        assert_eq!(count, 8);
    }
    #[test]
    fn group_mutations_return_their_accepted_owners_before_and_after_application() {
        use oer_ieee80211_port::conformance::group_keys::{Mutation, ProgressCase};
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c; 16],
            basic_rates_len: 1,
        };
        for mutation in Mutation::ALL {
            for applied in [false, true] {
                let Trace::Group(trace) = model(
                    Case::Group(ProgressCase::Disable { mutation, applied }),
                    &prepared,
                    6,
                )
                .expect("group trace") else {
                    panic!("group progress")
                };
                assert!(trace.within_bound);
                assert_eq!(trace.events.iter().flatten().count(), 1);
                assert!(!trace.poisoned);
            }
        }
        let Trace::Group(trace) = model(Case::Group(ProgressCase::PublishedRemoval), &prepared, 6)
            .expect("published group trace")
        else {
            panic!("group progress")
        };
        assert!(trace.blocked_while_published);
        assert_eq!(trace.events.iter().flatten().count(), 2);
    }
    #[test]
    fn configuration_contexts_return_in_fifo_order_across_every_disable_point() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c; 16],
            basic_rates_len: 1,
        };
        for case in oer_ieee80211_port::conformance::interface_commands::Case::all() {
            let Trace::Configure(trace) =
                model(Case::Configure(case), &prepared, 6).expect("configuration trace")
            else {
                panic!("configuration")
            };
            assert_eq!(trace.returned.iter().flatten().count(), 4);
            assert_eq!(trace.premature_drops, 0);
            assert!(!trace.extra_terminal);
        }
    }
    #[test]
    fn actual_model_outcomes_are_distinct_and_keep_owner_identity() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            basic_rates_len: 1,
        };
        let Trace::Outcome(ack) = model(Case::Outcome(outcomes::Case::Ack), &prepared, 6).unwrap()
        else {
            panic!("outcome trace")
        };
        let Trace::Outcome(absent) = model(
            Case::Outcome(outcomes::Case::NoResponse(outcomes::Form::Single)),
            &prepared,
            6,
        )
        .unwrap() else {
            panic!("outcome trace")
        };
        assert_eq!(ack.end, End::Ack);
        assert_eq!(absent.end, End::NoResponse);
        assert_eq!(ack.admitted, ack.returned);
        assert_eq!(ack.publications, 1);
        assert_ne!(ack, absent);
    }

    #[test]
    fn close_gate_ownership_exhausts_both_published_plans_without_a_default_ack() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            basic_rates_len: 1,
        };
        let Trace::Ownership(trace) = model(
            Case::Ownership(oer_ieee80211_port::conformance::Scenario::CloseGate),
            &prepared,
            6,
        )
        .expect("absent-peer ownership trace") else {
            panic!("ownership trace")
        };
        assert_eq!(trace.publications, 4);
        for ticket in [2, 3] {
            assert_eq!(
                trace
                    .publication_events
                    .iter()
                    .flatten()
                    .filter(|&&event| event == OwnerPublication::Tx(ticket))
                    .count(),
                2
            );
        }
        for row in trace.returned.iter().flatten() {
            assert!(row.body_pointer_matches);
            if row.actual.ticket >= 2 {
                assert_eq!(row.outcome, 2); // AckTimeout, from the actual terminal.
                assert_eq!(row.transmissions, Some(2));
            } else {
                assert_eq!(row.outcome, 6); // Withdrawn, without any publication.
                assert_eq!(row.transmissions, None);
            }
        }
    }

    #[test]
    fn model_ownership_fixtures_use_real_returned_resources_for_every_physical_scenario() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            basic_rates_len: 1,
        };
        for scenario in oer_ieee80211_port::conformance::Scenario::ALL {
            if scenario.execution() != Execution::Hardware {
                continue;
            }
            let Trace::Ownership(trace) = model(Case::Ownership(scenario), &prepared, 6).unwrap()
            else {
                panic!("ownership trace")
            };
            assert_eq!(trace.contexts_dropped, 4);
            assert_eq!(trace.returned.iter().flatten().count(), 4);
            for row in trace.returned.iter().flatten() {
                assert!(row.body_pointer_matches);
                if scenario == oer_ieee80211_port::conformance::Scenario::Refuse {
                    assert_eq!(row.expected.body, Some(0));
                    assert_eq!(row.actual.body, Some(0));
                }
            }
        }
    }
    #[test]
    fn every_physical_model_barrier_passes_the_shared_contract() {
        let prepared = Prepared {
            ampdu: false,
            basic_rates: [0x8c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            basic_rates_len: 1,
        };
        for case in oer_ieee80211_port::conformance::BarrierCase::ALL {
            if case.execution() == Execution::Hardware {
                assert!(matches!(
                    model(Case::Barrier(case), &prepared, 6).unwrap(),
                    Trace::Barrier(_)
                ));
            }
        }
    }
}
