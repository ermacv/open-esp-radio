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

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    context.results.claim("lower-mac-conformance", &[]);
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
            case.execution() == Execution::Hardware
                && case.extension() == Some(Extension::Ampdu) && !prepared.ampdu
        }).map(|(index, case)| {
            let reason = format!("{case:?}: не поддерживается бэкендом: нет AmpduPhase; до W6");
            context.results.observe(format!("gate-e-skipped-{index:02}"),
                &serde_json::json!({"status":"skipped","case":index,"reason":"backend has no AmpduPhase","until":"W6"}));
            eprintln!("гейт e: пропущено: {reason}");
            reason
        }).collect();
        for (index,case) in Case::all().enumerate() {
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
            if hardware.trace != model {
                context.results.observe("gate-e", &serde_json::json!({"pass":false,"case":index,"reason":"trace mismatch"}));
                return Err(format!("lower-MAC trace differs from ModelBackend in {case:?}; stop for owner/architect decision").into());
            }
            passed += 1;
        }
        context.results.observe("gate-e", &serde_json::json!({"pass":true,"passed":passed,"model_only":model_only,
            "unsupported":unsupported}));
        eprintln!("lower-MAC gate e: {passed} hardware traces equal ModelBackend; {model_only} model-only");
        Ok(())
    })
}
fn model(case: Case, prepared: &Prepared, channel: u8) -> Result<Trace> {
    let rates = prepared
        .basic_rates
        .get(..usize::from(prepared.basic_rates_len))
        .ok_or("invalid basic-rate extent")?;
    let basic = BasicRates::from_rate_elements(rates, &[]);
    let rate = management_rate(Band::Ghz2_4, Some(&basic)).ok_or("no basic management rate")?;
    let channel = Channel::ghz2_4(channel, ChannelWidth::Mhz20).map_err(|_| "invalid channel")?;
    if let Case::Ownership(scenario) = case {
        let raw = complete(oer_ieee80211_model::conformance::ownership(
            scenario,
            channel,
            rate,
            Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        ))??;
        let trace = ownership_trace(&raw).ok_or("unrepresentable model ownership")?;
        oer_ieee80211_port::conformance::verify(scenario, raw)
            .map_err(|why| format!("model ownership conformance violation: {why:?}"))?;
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
        }
    }
}
