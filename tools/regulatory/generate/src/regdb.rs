//! The `db.txt` grammar of wireless-regdb: `country XX: DFS-REGION` headers,
//! each followed by `(low - high @ bandwidth), (power), FLAG, …` rules.

use std::collections::BTreeMap;

/// One rule of a country, in kHz and mBm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rule {
    pub low_khz: u32,
    pub high_khz: u32,
    pub max_bandwidth_khz: u32,
    pub max_eirp_mbm: i32,
    pub no_ir: bool,
    pub dfs: bool,
    pub no_outdoor: bool,
}

/// The rules of the `wanted` countries in `text`, by country code, sorted by
/// frequency. The rules of other countries are not read.
pub fn parse(text: &str, wanted: &[&str]) -> Result<BTreeMap<String, Vec<Rule>>, String> {
    let mut countries: BTreeMap<String, Vec<Rule>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for (number, raw) in text.lines().enumerate() {
        let context = |message: String| format!("line {}: {message}", number + 1);
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("country ") {
            let code = rest.split(':').next().unwrap_or("").trim();
            current = Some(code.to_string());
            if wanted.contains(&code) {
                countries.entry(code.to_string()).or_default();
            }
        } else if line.starts_with('(') {
            let code = current
                .as_ref()
                .ok_or_else(|| context("rule before any country".into()))?;
            if !countries.contains_key(code) {
                continue;
            }
            let rule = parse_rule(line).map_err(context)?;
            if let Some(rules) = countries.get_mut(code) {
                rules.push(rule);
            }
        } else if line.starts_with("wmmrule ")
            || line.starts_with("vo_")
            || line.starts_with("vi_")
            || line.starts_with("be_")
            || line.starts_with("bk_")
        {
            // WMM parameter blocks are not part of the domain.
        } else {
            return Err(context(format!("unrecognised line `{line}`")));
        }
    }
    for rules in countries.values_mut() {
        rules.sort_by_key(|rule| (rule.low_khz, rule.high_khz));
    }
    Ok(countries)
}

fn parse_rule(line: &str) -> Result<Rule, String> {
    let mut parts = line.split("),").map(str::trim);
    let frequency = parts.next().ok_or("missing frequency")?;
    let power = parts.next().ok_or("missing power")?;
    let flags = parts.next();
    let frequency = frequency.trim_start_matches('(');
    let (span, bandwidth) = frequency.split_once('@').ok_or("missing `@`")?;
    let (low, high) = span.split_once('-').ok_or("missing `-`")?;
    let power = power.trim_start_matches('(').trim_end_matches(')').trim();
    let max_eirp_mbm = if let Some(milliwatts) = power.strip_suffix("mW") {
        let milliwatts: f64 = milliwatts
            .trim()
            .parse()
            .map_err(|_| format!("bad power `{power}`"))?;
        if milliwatts <= 0.0 {
            return Err(format!("bad power `{power}`"));
        }
        (milliwatts.log10() * 1000.0).round() as i32
    } else {
        let dbm: f64 = power.parse().map_err(|_| format!("bad power `{power}`"))?;
        (dbm * 100.0).round() as i32
    };
    let mut rule = Rule {
        low_khz: khz(low)?,
        high_khz: khz(high)?,
        max_bandwidth_khz: khz(bandwidth.trim_end_matches(')'))?,
        max_eirp_mbm,
        no_ir: false,
        dfs: false,
        no_outdoor: false,
    };
    if let Some(flags) = flags {
        for flag in flags.trim_end_matches(')').split(',').map(str::trim) {
            match flag {
                "NO-IR" => rule.no_ir = true,
                "DFS" => rule.dfs = true,
                "NO-OUTDOOR" => rule.no_outdoor = true,
                "AUTO-BW" => {}
                other if other.starts_with("wmmrule=") => {}
                other => return Err(format!("unknown flag `{other}`")),
            }
        }
    }
    if rule.low_khz >= rule.high_khz {
        return Err("empty frequency span".into());
    }
    Ok(rule)
}

/// MHz with up to three decimals, as kHz.
fn khz(text: &str) -> Result<u32, String> {
    let text = text.trim();
    let bad = || format!("bad frequency `{text}`");
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 {
        return Err(bad());
    }
    let whole: u32 = whole.parse().map_err(|_| bad())?;
    let mut fraction_khz: u32 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().map_err(|_| bad())?
    };
    for _ in fraction.len()..3 {
        fraction_khz *= 10;
    }
    whole
        .checked_mul(1000)
        .and_then(|whole| whole.checked_add(fraction_khz))
        .ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
wmmrule ETSI:
\tvo_c: cw_min=3, cw_max=7, aifsn=2, cot=2

country DE: DFS-ETSI
\t(2400 - 2483.5 @ 40), (100 mW)
\t(5250 - 5350 @ 80), (100 mW), NO-OUTDOOR, DFS, AUTO-BW, wmmrule=ETSI
\t# comment
\t(5945 - 6425 @ 320), (23), NO-IR
";

    #[test]
    fn parses_frequencies_powers_and_kept_flags() {
        let countries = parse(SAMPLE, &["DE"]).unwrap();
        let rules = &countries["DE"];
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].low_khz, 2_400_000);
        assert_eq!(rules[0].high_khz, 2_483_500);
        assert_eq!(rules[0].max_bandwidth_khz, 40_000);
        assert_eq!(rules[0].max_eirp_mbm, 2000);
        assert!(!rules[0].dfs && !rules[0].no_ir && !rules[0].no_outdoor);
        assert!(rules[1].dfs && rules[1].no_outdoor && !rules[1].no_ir);
        assert_eq!(rules[2].max_eirp_mbm, 2300);
        assert!(rules[2].no_ir);
    }

    #[test]
    fn rules_come_out_sorted_by_frequency() {
        let countries = parse(
            "country XX:\n\t(5000 - 5100 @ 20), (10)\n\t(2400 - 2483.5 @ 40), (10)\n",
            &["XX"],
        )
        .unwrap();
        assert_eq!(countries["XX"][0].low_khz, 2_400_000);
    }

    #[test]
    fn rules_of_other_countries_are_not_read() {
        let countries = parse(
            "country XX:\n\t(2400 - 2483.5 @ 40), (10), NO-OFDM\ncountry YY:\n\t(2400 - 2483.5 @ 40), (10)\n",
            &["YY"],
        )
        .unwrap();
        assert_eq!(countries.len(), 1);
    }

    #[test]
    fn an_unknown_flag_stops_the_run() {
        let error = parse(
            "country XX:\n\t(2400 - 2483.5 @ 40), (10), NO-HT40\n",
            &["XX"],
        )
        .unwrap_err();
        assert!(error.contains("NO-HT40"), "{error}");
    }

    #[test]
    fn an_unrecognised_line_stops_the_run() {
        assert!(parse("country XX:\n\tbogus\n", &["XX"]).is_err());
        assert!(parse("(2400 - 2483.5 @ 40), (10)\n", &["XX"]).is_err());
    }
}
