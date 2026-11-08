use super::*;

/// A router's mac80211 debugfs as these scripts use it: `aql_txq_limit`
/// takes `<ac> <low> <high>` and prints every category; `aql_threshold`
/// prints one number; a station's `aql` prints its pending airtime and
/// limits. `ignore_writes` models a router that does not apply a write.
struct Router {
    limits: [Limit; 4],
    threshold_us: u32,
    station: Option<([u32; 4], [Limit; 4])>,
    ignore_writes: bool,
    has_aql: bool,
    scripts: usize,
}

impl Router {
    fn at_defaults() -> Self {
        Self {
            limits: [DEFAULT; 4],
            threshold_us: 24000,
            station: None,
            ignore_writes: false,
            has_aql: true,
            scripts: 0,
        }
    }

    fn print_limits(&self) -> String {
        let mut text = String::from("AC\tAQL limit low\tAQL limit high\n");
        for (ac, limit) in AccessCategory::ALL.iter().zip(self.limits) {
            text.push_str(&format!(
                "{}\t{}\t\t{}\n",
                ac.label(),
                limit.low_us,
                limit.high_us
            ));
        }
        text.push_str("BC/MC\t50000\n");
        text
    }
}

impl Remote for Router {
    fn run(&mut self, script: &str) -> Result<String> {
        self.scripts += 1;
        if script.contains("then echo present; else echo absent; fi") {
            return Ok(if self.has_aql {
                "present\n"
            } else {
                "absent\n"
            }
            .to_owned());
        }
        let mut out = String::new();
        for line in script.lines() {
            if let Some(write) = line
                .strip_prefix("echo '")
                .and_then(|rest| rest.strip_suffix("' > \"$d/aql_txq_limit\""))
            {
                let values: Vec<u32> = write.split(' ').map(|v| v.parse().unwrap()).collect();
                if !self.ignore_writes {
                    self.limits[values[0] as usize] = Limit {
                        low_us: values[1],
                        high_us: values[2],
                    };
                }
            } else if let Some(name) = line
                .strip_prefix("echo '")
                .and_then(|r| r.strip_suffix('\''))
            {
                out.push_str(name);
                out.push('\n');
            } else if line == "cat \"$d/aql_txq_limit\"" {
                out.push_str(&self.print_limits());
            } else if line == "cat \"$d/aql_threshold\"" {
                out.push_str(&format!("{}\n", self.threshold_us));
            } else if line.starts_with("cat \"$d/netdev:") {
                let (pending, limits) = self.station.ok_or("no such station file")?;
                out.push_str(&format!(
                    "Q depth: VO: {} us VI: {} us BE: {} us BK: {} us\n\
                     Q limit[low/high]: VO: {}/{} VI: {}/{} BE: {}/{} BK: {}/{}\n",
                    pending[0],
                    pending[1],
                    pending[2],
                    pending[3],
                    limits[0].low_us,
                    limits[0].high_us,
                    limits[1].low_us,
                    limits[1].high_us,
                    limits[2].low_us,
                    limits[2].high_us,
                    limits[3].low_us,
                    limits[3].high_us,
                ));
            }
        }
        Ok(out)
    }
}

const RAISED: Limit = Limit {
    low_us: 5000,
    high_us: 18000,
};

#[test]
fn a_set_changes_only_its_category_and_reads_back_every_limit() {
    let mut router = Router::at_defaults();
    let read = set(&mut router, "radio0", AccessCategory::Be, RAISED).unwrap();
    assert_eq!(read.radio.be, RAISED);
    assert_eq!(read.radio.vo, DEFAULT);
    assert_eq!(read.radio.vi, DEFAULT);
    assert_eq!(read.radio.bk, DEFAULT);
    assert_eq!(read.threshold_us, 24000);
    assert_eq!(read.station, None);
    assert_eq!(router.limits[2], RAISED);
}

#[test]
fn a_write_the_router_did_not_apply_is_an_error_not_a_success() {
    let mut router = Router {
        ignore_writes: true,
        ..Router::at_defaults()
    };
    assert!(set(&mut router, "radio0", AccessCategory::Be, RAISED).is_err());
    assert!(
        restore_defaults(
            &mut Router {
                limits: [RAISED; 4],
                ignore_writes: true,
                ..Router::at_defaults()
            },
            "radio0"
        )
        .is_err()
    );
}

#[test]
fn a_limit_outside_the_lab_bounds_never_reaches_the_router() {
    for limit in [
        Limit {
            low_us: 0,
            high_us: 12000,
        },
        Limit {
            low_us: 13000,
            high_us: 12000,
        },
        Limit {
            low_us: 5000,
            high_us: MAX_HIGH_US + 1,
        },
    ] {
        let mut router = Router::at_defaults();
        assert!(set(&mut router, "radio0", AccessCategory::Be, limit).is_err());
        assert_eq!(router.scripts, 0, "{limit:?}");
        assert_eq!(router.limits, [DEFAULT; 4]);
    }
}

#[test]
fn restoring_returns_every_category_to_the_defaults_and_confirms_it() {
    let mut router = Router {
        limits: [
            RAISED,
            Limit {
                low_us: 1000,
                high_us: 2000,
            },
            RAISED,
            RAISED,
        ],
        ..Router::at_defaults()
    };
    let read = restore_defaults(&mut router, "radio0").unwrap();
    assert_eq!(router.limits, [DEFAULT; 4]);
    for ac in AccessCategory::ALL {
        assert_eq!(read.radio.get(ac), DEFAULT);
    }
}

#[test]
fn a_station_read_names_its_own_limits_and_pending_airtime() {
    let mut router = Router {
        limits: [DEFAULT, DEFAULT, RAISED, DEFAULT],
        station: Some(([0, 0, 12052, 0], [DEFAULT, DEFAULT, DEFAULT, DEFAULT])),
        ..Router::at_defaults()
    };
    let mac = Mac::parse("02:00:00:00:00:01").unwrap();
    let read = read(&mut router, "radio0", Some(("phy0-ap0", mac))).unwrap();
    // The radio's category and the station's own may differ: a station with
    // customized limits keeps them when the radio's change.
    assert_eq!(read.radio.be, RAISED);
    let station = read.station.unwrap();
    assert_eq!(station.limits.be, DEFAULT);
    assert_eq!(station.pending_us.be, 12052);
}

#[test]
fn an_unreadable_answer_is_an_error() {
    for text in [
        "",
        "[limits]\nVO\t5000\t\t12000\n[threshold]\n24000\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\nBE 1 2\n[threshold]\n24000\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE x 2\nBK 1 2\n[threshold]\n24000\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\n[threshold]\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\n[threshold]\n24000\n[station]\nQ depth: VO: 0 us\n",
        "stray\n[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\n[threshold]\n24000\n",
    ] {
        assert!(parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn a_radio_says_whether_it_has_airtime_limits() {
    assert!(present(&mut Router::at_defaults(), "radio0").unwrap());
    assert!(
        !present(
            &mut Router {
                has_aql: false,
                ..Router::at_defaults()
            },
            "radio0"
        )
        .unwrap()
    );
}
