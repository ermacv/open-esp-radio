use super::*;
use crate::channel::ChannelWidth;

fn domain(code: &str) -> RegulatoryDomain {
    RegulatoryDomain::for_country(CountryCode::parse(code).unwrap()).unwrap()
}

fn mhz20(primary: u8) -> WifiChannel {
    WifiChannel::mhz20(primary).unwrap()
}

#[test]
fn country_codes_are_two_letters_or_the_world_code() {
    assert_eq!(CountryCode::parse("de").unwrap().bytes(), *b"DE");
    assert_eq!(CountryCode::parse("00"), Ok(CountryCode::WORLD));
    for text in ["", "D", "DEU", "D1", "0A", "é", "\u{0}\u{0}"] {
        assert_eq!(CountryCode::parse(text), Err(CountryCodeError), "{text:?}");
    }
    assert_eq!(std::format!("{}", CountryCode::parse("ru").unwrap()), "RU");
}

#[test]
fn an_unlisted_country_is_refused_not_given_other_rules() {
    let code = CountryCode::parse("ZZ").unwrap();
    assert_eq!(
        RegulatoryDomain::for_country(code),
        Err(UnlistedCountry(code))
    );
}

#[test]
fn the_world_domain_allows_channels_1_to_11_at_20_dbm() {
    let world = RegulatoryDomain::for_country(CountryCode::WORLD).unwrap();
    assert_eq!(world, RegulatoryDomain::WORLD);
    for channel in 1..=11 {
        let legal = world.legal(mhz20(channel)).unwrap();
        assert_eq!(legal.channel(), mhz20(channel));
        assert_eq!(legal.max_eirp_dbm(), 20);
        assert_eq!(legal.country(), CountryCode::WORLD);
    }
    for channel in 12..=14 {
        assert_eq!(
            world.legal(mhz20(channel)),
            Err(IllegalChannel::Disallowed {
                country: CountryCode::WORLD,
                channel: mhz20(channel),
            })
        );
    }
}

#[test]
fn each_listed_country_allows_its_channels_and_ceiling() {
    for (code, last, eirp) in [("DE", 13, 20), ("RU", 13, 20), ("US", 11, 30)] {
        let domain = domain(code);
        assert_eq!(domain.ghz2_4().first_channel(), 1);
        assert_eq!(domain.ghz2_4().last_channel(), last);
        assert_eq!(domain.ghz2_4().max_eirp_dbm(), eirp);
        assert_eq!(domain.legal(mhz20(last)).unwrap().max_eirp_dbm(), eirp);
        assert!(domain.legal(mhz20(last + 1)).is_err(), "{code}");
    }
    // Channel 14 is allowed nowhere in the table.
    for code in ["00", "DE", "RU", "US"] {
        assert!(domain(code).legal(mhz20(14)).is_err());
    }
}

#[test]
fn a_40_mhz_channel_is_legal_only_when_its_secondary_is() {
    let us = domain("US");
    let above = WifiChannel::new_2_4_ghz(7, ChannelWidth::Mhz40Above).unwrap();
    assert_eq!(us.legal(above).unwrap().channel(), above);
    let past = WifiChannel::new_2_4_ghz(9, ChannelWidth::Mhz40Above).unwrap();
    assert_eq!(
        us.legal(past),
        Err(IllegalChannel::Disallowed {
            country: us.country(),
            channel: past,
        })
    );
    let below = WifiChannel::new_2_4_ghz(13, ChannelWidth::Mhz40Below).unwrap();
    assert!(us.legal(below).is_err());
    assert!(domain("DE").legal(below).is_ok());
}

#[test]
fn the_ceiling_is_the_lower_of_the_limit_and_the_cap() {
    let legal = domain("DE").legal(mhz20(6)).unwrap();
    assert_eq!(legal.capped(10).max_eirp_dbm(), 10);
    // A cap above the country's limit leaves the limit.
    assert_eq!(legal.capped(27).max_eirp_dbm(), 20);
    assert_eq!(legal.capped(20).max_eirp_dbm(), 20);
    // Caps only lower: a second, higher cap does not undo the first.
    assert_eq!(legal.capped(10).capped(15).max_eirp_dbm(), 10);
    assert_eq!(legal.capped(10).channel(), legal.channel());
}

#[test]
fn country_parsing_never_panics_on_host_input() {
    crate::fuzz::inputs(&[b"DE", b"00", b"us"], crate::fuzz::QUICK, 4, |bytes| {
        if let Ok(text) = core::str::from_utf8(bytes)
            && let Ok(code) = CountryCode::parse(text)
            && let Ok(domain) = RegulatoryDomain::for_country(code)
        {
            for primary in 1..=14 {
                let _ = domain.legal(mhz20(primary));
            }
        }
        if let [first, second] = *bytes {
            let _ = CountryCode::new([first, second]);
        }
    });
}
