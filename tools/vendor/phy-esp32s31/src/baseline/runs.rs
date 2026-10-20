//! Import authenticated run attachments; raw bundles remain outside git.
use super::*;
use serde_json::Value;

fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}").into())
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing {key}").into())
}
fn observation<'a>(observations: &'a Value, name: &str) -> Result<&'a Value> {
    let mut found = array(observations, "observations")?
        .iter()
        .filter(|v| v.get("name").and_then(Value::as_str) == Some(name));
    let first = found
        .next()
        .and_then(|v| v.get("value"))
        .ok_or_else(|| format!("missing observation {name}"))?;
    if found.next().is_some() {
        return Err(format!("duplicate observation {name}").into());
    }
    Ok(first)
}
fn attached(root: &Path, repetition: &Value, relative: &str) -> Result<Vec<u8>> {
    let mut attachments = array(repetition, "attachments")?
        .iter()
        .filter(|a| a.get("path").and_then(Value::as_str) == Some(relative));
    let a = attachments
        .next()
        .ok_or_else(|| format!("missing authenticated attachment {relative}"))?;
    if attachments.next().is_some()
        || Path::new(relative)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("duplicate or unsafe attachment path".into());
    }
    let bytes = std::fs::read(root.join(relative))?;
    if text(a, "sha256")? != oer_os_durable::sha256_bytes(&bytes)
        || a.get("size_bytes").and_then(Value::as_u64) != Some(bytes.len() as u64)
    {
        return Err(format!("attachment changed: {relative}").into());
    }
    Ok(bytes)
}
fn manifest(root: &Path, label: &str) -> Result<Value> {
    let manifest = read(&root.join("manifest.json"))?;
    if !super::label(label)
        || text(&manifest, "target")? != CHIP
        || text(&manifest, "state")? != "completed"
        || manifest.pointer("/cell/device_id").and_then(Value::as_str) != Some(label)
    {
        return Err("run is incomplete or targets another chip or board label".into());
    }
    Ok(manifest)
}
fn observations(root: &Path, repetition: &Value) -> Result<(Value, String)> {
    let directory = text(repetition, "artifact_directory")?;
    let bytes = attached(root, repetition, &format!("{directory}/observations.json"))?;
    let observations: Value = serde_json::from_slice(&bytes)?;
    if observations.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("unknown observations schema".into());
    }
    let cleanup: Value = serde_json::from_slice(&attached(
        root,
        repetition,
        &format!("{directory}/cleanup.json"),
    )?)?;
    if cleanup.as_array().is_none_or(|a| !a.is_empty()) {
        return Err("run cleanup is incomplete".into());
    }
    Ok((observations, oer_os_durable::sha256_bytes(&bytes)))
}
fn count(observations: &Value, prefix: &str) -> Result<usize> {
    let names = array(observations, "observations")?
        .iter()
        .filter_map(|v| v.get("name").and_then(Value::as_str))
        .filter(|n| n.starts_with(prefix))
        .collect::<Vec<_>>();
    for i in 1..=names.len() {
        observation(observations, &format!("{prefix}{i:03}"))?;
    }
    Ok(names.len())
}

/// Only complete cold primary series are imported; comparison DIFF is allowed.
pub fn vendor(root: &Path, label: &str, revision: u16) -> Result<Vec<Input>> {
    let manifest = manifest(root, label)?;
    let suite = read(&root.join("suite.json"))?;
    let mut inputs = vec![];
    if suite.get("run_id") != manifest.get("run_id") {
        return Err("suite/run identity differs".into());
    }
    for scenario in array(&suite, "scenarios")? {
        let name = text(scenario, "scenario")?;
        if !matches!(
            name,
            "phy-vendor-calibration-cold" | "phy-vendor-calibration-cold-vendor-spread"
        ) {
            continue;
        }
        for rep in array(scenario, "repetitions")? {
            if !matches!(text(rep, "outcome")?, "passed" | "failed") {
                return Err("reference repetition did not complete".into());
            }
            let (observations, sha) = observations(root, rep)?;
            let identity: Images =
                serde_json::from_value(observation(&observations, "images")?.clone())?;
            let directory = text(rep, "artifact_directory")?;
            let application = attached(
                root,
                rep,
                &format!("{directory}/vendor-image/application.bin"),
            )?;
            if oer_os_durable::sha256_bytes(&application) != identity.vendor_application_sha256 {
                return Err("vendor application differs from its recorded identity".into());
            }
            let n = count(&observations, "vendor-boot-")?;
            if n == 0 {
                return Err("no primary reference boots".into());
            }
            if let Ok(comparison) = observation(&observations, "comparison") {
                if comparison.get("lifecycle").and_then(Value::as_str) != Some("cold")
                    || comparison.get("vendor-captures").and_then(Value::as_u64) != Some(n as u64)
                {
                    return Err("incomplete or non-cold comparison series".into());
                }
            } else if observations
                .pointer("/claim/result")
                .and_then(Value::as_str)
                != Some("vendor boot spread captured")
                || text(rep, "outcome")? != "passed"
            {
                return Err("vendor-only series has no completion claim".into());
            }
            let discovery: VendorBoot = serde_json::from_slice(&attached(
                root,
                rep,
                &format!("{directory}/vendor-readability.json"),
            )?)?;
            let mut boots = vec![];
            let mut channel = None;
            for i in 1..=n {
                let recorded = observation(&observations, &format!("vendor-boot-{i:03}"))?;
                let bytes = attached(root, rep, &format!("{directory}/vendor-{i:03}.json"))?;
                if serde_json::from_slice::<Value>(&bytes)? != *recorded {
                    return Err("primary boot differs from its observation".into());
                }
                let boot: VendorBoot = serde_json::from_slice(&bytes)?;
                if boot.registers.unreadable != discovery.registers.unreadable
                    || boot
                        .registers
                        .values
                        .keys()
                        .ne(discovery.registers.values.keys())
                    || boot.analog.unreadable != discovery.analog.unreadable
                    || boot.analog.values.keys().ne(discovery.analog.values.keys())
                {
                    return Err("primary readability differs from discovery".into());
                }
                let c = u8::try_from(
                    observation(&observations, &format!("vendor-channel-{i:03}"))?
                        .as_u64()
                        .ok_or("missing vendor channel")?,
                )?;
                if channel.replace(c).is_some_and(|old| old != c) {
                    return Err("reference channel changed".into());
                }
                boots.push(boot);
            }
            inputs.push(Input {
                label: label.into(),
                revision,
                source: Source {
                    run_id: text(&manifest, "run_id")?.into(),
                    scenario: name.into(),
                    repetition: u32::try_from(
                        rep.get("repetition")
                            .and_then(Value::as_u64)
                            .ok_or("missing repetition")?,
                    )?,
                    repository_commit: text(
                        manifest.get("repository").ok_or("missing repository")?,
                        "commit",
                    )?
                    .into(),
                    repository_dirty: manifest
                        .pointer("/repository/dirty")
                        .and_then(Value::as_bool)
                        .ok_or("missing repository dirty flag")?,
                    date: compare::utc_date(identity.started_unix_seconds),
                    samples: n,
                    observations_sha256: sha,
                },
                identity,
                channel: channel.ok_or("missing channel")?,
                boots,
            });
        }
    }
    if inputs.is_empty() {
        return Err("run contains no complete Wi-Fi cold vendor series".into());
    }
    Ok(inputs)
}

/// Read complete production captures from a cold paired or recorded-reference run.
fn production_series(repetition: &Value, observations: &Value, captures: usize) -> Result<()> {
    let comparison = observation(observations, "comparison")?;
    let summary = comparison.get("comparison").unwrap_or(comparison);
    if !matches!(text(repetition, "outcome")?, "passed" | "failed")
        || !matches!(text(comparison, "verdict")?, "match" | "diff")
        || summary.get("production-captures").and_then(Value::as_u64) != Some(captures as u64)
        || captures == 0
    {
        return Err("production series did not complete its capture/comparison".into());
    }
    Ok(())
}

pub fn production(root: &Path, label: &str, channel: u8) -> Result<(Images, Vec<ProductionBoot>)> {
    manifest(root, label)?;
    let suite = read(&root.join("suite.json"))?;
    let mut captures = vec![];
    let mut identity = None;
    for scenario in array(&suite, "scenarios")? {
        if !matches!(
            text(scenario, "scenario")?,
            "phy-vendor-calibration-cold"
                | "phy-vendor-calibration-cold-smoke"
                | "phy-recorded-calibration-cold"
        ) {
            continue;
        }
        for rep in array(scenario, "repetitions")? {
            let (observations, _) = observations(root, rep)?;
            let image: Images =
                serde_json::from_value(observation(&observations, "images")?.clone())?;
            if identity.as_ref().is_some_and(|previous| previous != &image) {
                return Err("production image identity changes between repetitions".into());
            }
            identity = Some(image);
            let directory = text(rep, "artifact_directory")?;
            let n = count(&observations, "production-boot-")?;
            production_series(rep, &observations, n)?;
            for i in 1..=n {
                let ready = observation(&observations, &format!("production-{i:03}-registration"))?;
                if ready.get("lifecycle").and_then(Value::as_str) != Some("Cold")
                    || ready.get("radio").and_then(Value::as_str) != Some("Wifi")
                    || ready.get("path").and_then(Value::as_str) != Some("FullForCache")
                    || ready
                        .get("full_calibration_performed")
                        .and_then(Value::as_bool)
                        != Some(true)
                    || ready.get("channel").and_then(Value::as_u64) != Some(u64::from(channel))
                    || ready.get("projection_schema").and_then(Value::as_u64) != Some(1)
                {
                    return Err("production capture has a different lifecycle/channel or incomplete registration".into());
                }
                let bytes = attached(
                    root,
                    rep,
                    &format!("{directory}/production-{i:03}/capture.json"),
                )?;
                if serde_json::from_slice::<Value>(&bytes)?
                    != *observation(&observations, &format!("production-boot-{i:03}"))?
                {
                    return Err("production capture differs from its observation".into());
                }
                captures.push(serde_json::from_slice(&bytes)?);
            }
        }
    }
    Ok((identity.ok_or("no production image identity")?, captures))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_production_series_cannot_become_a_match_from_the_completed_prefix() {
        let mut rep = serde_json::json!({"outcome":"failed"});
        let mut observations = serde_json::json!({"observations":[{"name":"comparison","value":{"verdict":"diff","production-captures":2}}]});
        production_series(&rep, &observations, 2).unwrap();
        assert!(production_series(&rep, &observations, 1).is_err());
        rep["outcome"] = "broken".into();
        assert!(production_series(&rep, &observations, 2).is_err());
        rep["outcome"] = "failed".into();
        observations["observations"][0]["value"]["verdict"] = "incomplete".into();
        assert!(production_series(&rep, &observations, 2).is_err());
    }
    #[test]
    fn attachments_cannot_change_or_escape_and_discovery_is_not_a_primary_series() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("boot.json"), b"{}").unwrap();
        let r = serde_json::json!({"attachments":[{"path":"boot.json","size_bytes":2,"sha256":oer_os_durable::sha256_bytes(b"{}")} ]});
        assert_eq!(attached(dir.path(), &r, "boot.json").unwrap(), b"{}");
        std::fs::write(dir.path().join("boot.json"), b"[]").unwrap();
        assert!(attached(dir.path(), &r, "boot.json").is_err());
        assert!(attached(dir.path(), &r, "../boot.json").is_err());
        assert_eq!(count(&serde_json::json!({"observations":[{"name":"vendor-readability-discovery","value":{}}]}),"vendor-boot-").unwrap(),0);
        assert!(
            count(
                &serde_json::json!({"observations":[{"name":"vendor-boot-002","value":{}}]}),
                "vendor-boot-"
            )
            .is_err()
        );
    }
}
