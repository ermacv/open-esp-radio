//! The station profile's validation and its generated constant; shared by
//! the build script and the crate's host tests.

const FIELDS: [&str; 6] = [
    "ssid",
    "security",
    "passphrase",
    "country",
    "channel",
    "cap_dbm",
];

/// The profile's Rust constant; each reason names a field, never its value.
pub fn parse(text: &str) -> Result<String, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|_| "the profile is not valid TOML".to_string())?;
    if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(format!("the profile has no field `{unknown}`"));
    }
    let text_field = |name: &str| -> Result<&str, String> {
        table
            .get(name)
            .ok_or_else(|| format!("`{name}` is required"))?
            .as_str()
            .ok_or_else(|| format!("`{name}` is a string"))
    };
    let integer = |name: &str| -> Result<i64, String> {
        table
            .get(name)
            .ok_or_else(|| format!("`{name}` is required"))?
            .as_integer()
            .ok_or_else(|| format!("`{name}` is an integer"))
    };
    let ssid = text_field("ssid")?.as_bytes();
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("`ssid` is 1 to 32 bytes".into());
    }
    let security = match text_field("security")? {
        "open" => {
            if table.contains_key("passphrase") {
                return Err("an `open` profile has no `passphrase`".into());
            }
            "crate::profile::Security::Open".to_string()
        }
        "wpa2-personal" => {
            let passphrase = text_field("passphrase")?.as_bytes();
            if !(8..=63).contains(&passphrase.len())
                || !passphrase.iter().all(|byte| (0x20..=0x7e).contains(byte))
            {
                return Err("`passphrase` is 8 to 63 printable ASCII characters".into());
            }
            format!(
                "crate::profile::Security::Wpa2Personal {{ passphrase: &[{}] }}",
                octets(passphrase)
            )
        }
        _ => return Err("`security` is `open` or `wpa2-personal`".into()),
    };
    let country = match text_field("country")?.as_bytes() {
        &[first, second] if first.is_ascii_uppercase() && second.is_ascii_uppercase() => {
            [first, second]
        }
        _ => return Err("`country` is two uppercase letters".into()),
    };
    let channel = u8::try_from(integer("channel")?)
        .ok()
        .filter(|channel| (1..=13).contains(channel))
        .ok_or("`channel` is a 2.4 GHz channel from 1 to 13")?;
    let cap_dbm = i8::try_from(integer("cap_dbm")?)
        .ok()
        .filter(|cap| (0..=20).contains(cap))
        .ok_or("`cap_dbm` is a transmit power limit from 0 to 20 dBm")?;
    Ok(format!(
        "/// The station this image is built for.\n\
         pub const PROFILE: crate::profile::StationProfile = crate::profile::StationProfile {{ \
         ssid: &[{}], security: {security}, country: [{:#04x}, {:#04x}], channel: {channel}, \
         cap_dbm: {cap_dbm} }};\n",
        octets(ssid),
        country[0],
        country[1],
    ))
}

/// `0x6f, 0x65, …`.
fn octets(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|octet| format!("{octet:#04x}"))
        .collect::<Vec<_>>()
        .join(", ")
}
