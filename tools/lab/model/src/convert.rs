//! The one rewrite of a schema 1 stand file to schema 2, keeping its
//! comments and layout. Schema 1 described the lab router as
//! `[station_fixture]` with the channel a run used; schema 2 describes it as
//! `[router]`, equipment of the LAB service, with the channels the stand may
//! use on it, and a station on this host as `[local_station]`. The access
//! point's channel and width become the ones it allows. Nothing is dropped:
//! a key this rewrite does not place refuses it. Errors name keys, never
//! values: the file holds credentials.

use toml_edit::{Array, DocumentMut, Item, Key, Value};

/// A schema 2 stand file and what the rewrite changed beyond renaming.
#[derive(Debug, Eq, PartialEq)]
pub struct Converted {
    pub text: String,
    pub notes: Vec<String>,
}

/// The keys of `[station_fixture]` that describe the lab router.
const ROUTER_KEYS: [&str; 10] = [
    "kind",
    "ssh_target",
    "radio",
    "ap_section",
    "wireless_interface",
    "ingress_interface",
    "monitor_interface",
    "phys",
    "independent_laptop_monitor",
    "read_only",
];
/// The keys of `[station_fixture]` that describe a station on this host.
const LOCAL_KEYS: [&str; 6] = [
    "kind",
    "interface",
    "country",
    "address",
    "coexistence",
    "read_only",
];

/// Whether `text` is a schema 1 stand file.
pub fn is_schema_1(text: &str) -> bool {
    text.parse::<DocumentMut>()
        .is_ok_and(|document| document.get("schema").and_then(Item::as_integer) == Some(1))
}

/// Rewrite the schema 1 stand file `text` to schema 2.
pub fn from_schema_1(text: &str) -> Result<Converted, String> {
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|error| format!("the stand file is no TOML: {}", error.message()))?;
    match document.get("schema").and_then(Item::as_integer) {
        Some(1) => {}
        Some(schema) => return Err(format!("the stand file has schema {schema}, not 1")),
        None => return Err("the stand file names no schema".to_owned()),
    }
    let mut notes = Vec::new();
    if let Some(schema) = document.get_mut("schema").and_then(Item::as_value_mut) {
        let decor = schema.decor().clone();
        *schema = Value::from(2_i64);
        *schema.decor_mut() = decor;
    }
    if let Some(fixture) = document.remove("station_fixture") {
        let fixture = fixture
            .into_table()
            .map_err(|_| "[station_fixture] is no table".to_owned())?;
        let kind = fixture
            .get("kind")
            .and_then(Item::as_str)
            .map(str::to_owned);
        let (name, keys) = match kind.as_deref() {
            Some("open-wrt") => ("router", &ROUTER_KEYS[..]),
            Some("local") => ("local_station", &LOCAL_KEYS[..]),
            Some(other) => {
                return Err(format!(
                    "[station_fixture] kind `{other}` has no schema 2 section"
                ));
            }
            None => return Err("[station_fixture] names no kind".to_owned()),
        };
        let mut section = fixture.clone();
        section.clear();
        for (key, item) in fixture.iter() {
            // A key keeps the comments written above it.
            let decor = fixture
                .key(key)
                .map(|key| key.leaf_decor().clone())
                .unwrap_or_default();
            let named = |name: &str| {
                let mut key = Key::new(name);
                *key.leaf_decor_mut() = decor.clone();
                key
            };
            match key {
                "channel" if name == "router" => {
                    let channel = item
                        .as_integer()
                        .ok_or("[station_fixture].channel is no number")?;
                    section.insert_formatted(&named("channels"), Item::Value(single(channel)));
                    notes.push(format!(
                        "[station_fixture].channel {channel} is the run's: [router].channels = \
                         [{channel}] allows it"
                    ));
                }
                "kind" if name == "local_station" => {}
                key if keys.contains(&key) => {
                    section.insert_formatted(&named(key), item.clone());
                }
                key => {
                    return Err(format!(
                        "[station_fixture].{key} has no place in [{name}] of schema 2"
                    ));
                }
            }
        }
        document.insert(name, Item::Table(section));
    }
    if let Some(access_point) = document
        .get_mut("access_point")
        .and_then(Item::as_table_mut)
    {
        for (from, to) in [("channel", "channels"), ("channel_width", "widths")] {
            if let Some((old, item)) = access_point.remove_entry(from) {
                let mut value = item
                    .into_value()
                    .map_err(|_| format!("[access_point].{from} is no value"))?;
                value.decor_mut().clear();
                let mut allowed = Array::new();
                allowed.push(value);
                let mut key = Key::new(to);
                *key.leaf_decor_mut() = old.leaf_decor().clone();
                access_point.insert_formatted(&key, Item::Value(Value::Array(allowed)));
                notes.push(format!(
                    "[access_point].{from} is the run's: [access_point].{to} allows it"
                ));
            }
        }
    }
    Ok(Converted {
        text: document.to_string(),
        notes,
    })
}

fn single(value: i64) -> Value {
    let mut array = Array::new();
    array.push(value);
    Value::Array(array)
}

/// The keys of `table`, for a test that compares sections.
#[cfg(test)]
fn keys(table: &toml_edit::Table) -> Vec<&str> {
    table.iter().map(|(key, _)| key).collect()
}

#[cfg(test)]
mod tests;
