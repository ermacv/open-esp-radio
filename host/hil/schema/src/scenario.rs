//! Canonical executable scenario values. Producers digest and evaluators
//! compare scenarios in this form, so both fill the same schema-5 defaults.
//!
//! The typed scenario families own the defaults. The defaults document is
//! generated from them by the runner's catalog test (set
//! `OER_HIL_BLESS_SCENARIO_DEFAULTS=1` to regenerate it), which fails while
//! the committed document differs; it is never edited by hand.
//!
//! The defaults document mirrors the scenario tree. A `$kind` entry maps a
//! table's `kind` discriminator to the defaults of that variant, and a key
//! prefixed with `?` names an optional table whose defaults apply only when
//! the table is present. Absent optional values and explicit nulls are the
//! same value, so nulls are removed before defaults are filled.
use serde_json::{Map, Value};

/// Remove presentation-only fields, drop nulls and fill schema-5 defaults.
///
/// Whether the current firmware can run a scenario, and whether a program
/// references it, are not part of its procedure: marking it unsupported or
/// changing its role keeps its recorded evidence comparable.
pub fn normalize(document: &Value) -> Value {
    let defaults: Value = serde_json::from_str(include_str!("../scenario-v5-defaults.json"))
        .expect("compiled scenario defaults must be valid JSON");
    let mut value = document.clone();
    remove_nulls(&mut value);
    fill(&mut value, &defaults);
    if let Some(object) = value.as_object_mut() {
        object.remove("description");
        object.remove("role");
        object.remove("tags");
        object.remove("unsupported");
    }
    value
}

fn remove_nulls(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|_, value| !value.is_null());
            object.values_mut().for_each(remove_nulls);
        }
        Value::Array(values) => values.iter_mut().for_each(remove_nulls),
        _ => {}
    }
}

fn fill(value: &mut Value, defaults: &Value) {
    let (Some(object), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) else {
        return;
    };
    for (key, default) in defaults {
        if key == "$kind" {
            continue;
        }
        if let Some(optional) = key.strip_prefix('?') {
            if let Some(present) = object.get_mut(optional) {
                fill(present, default);
            }
            continue;
        }
        let current = object
            .entry(key.clone())
            .or_insert_with(|| empty_like(default));
        fill(current, default);
    }
    let variant = object
        .get("kind")
        .and_then(Value::as_str)
        .and_then(|kind| defaults.get("$kind")?.get(kind));
    if let Some(variant) = variant {
        fill(value, variant);
    }
}

/// The value a missing entry starts from: scalars and arrays are the default
/// itself, while tables are filled key by key so markers are never copied.
fn empty_like(default: &Value) -> Value {
    match default {
        Value::Object(_) => Value::Object(Map::new()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn variant_and_optional_defaults_apply_only_where_selected() {
        let defaults = json!({"repetitions": 1, "?family": {"$kind": {
            "secure": {"shutdown": "bond-load-failure"}}}});
        let mut secure = json!({"family": {"kind": "secure"}});
        fill(&mut secure, &defaults);
        assert_eq!(secure["repetitions"], 1);
        assert_eq!(secure["family"]["shutdown"], "bond-load-failure");
        // Another variant holds none of the secure variant's defaults.
        let mut plain = json!({"family": {"kind": "plain"}});
        fill(&mut plain, &defaults);
        assert!(plain["family"].get("shutdown").is_none());
        // A document without the optional table gains none of it.
        let mut other = json!({"system": {"kind": "boot-smoke"}});
        fill(&mut other, &defaults);
        assert!(other.get("family").is_none());
    }

    #[test]
    fn presentation_fields_do_not_change_the_procedure() {
        let control = json!({"schema": 5, "id": "x", "description": "d", "tags": ["a"],
            "system": {"kind": "boot-smoke"}});
        let normalized = normalize(&control);
        assert!(normalized.get("description").is_none() && normalized.get("tags").is_none());
        let mut unsupported = control.clone();
        unsupported["unsupported"] = json!("no image serves it");
        assert_eq!(normalize(&unsupported), normalized);
    }
}
