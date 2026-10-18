//! Serde form of an ELF symbol name: a string when its bytes are UTF-8, as
//! nearly every name is, and its byte array otherwise, so no name is lost.
//!
//! Use with `#[serde(with = "oer_riscv_model::symbol_name")]` on a
//! `Vec<u8>` and `#[serde(with = "oer_riscv_model::symbol_name::option")]`
//! on an `Option<Vec<u8>>`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Form {
    Text(String),
    Bytes(Vec<u8>),
}

impl From<Form> for Vec<u8> {
    fn from(form: Form) -> Self {
        match form {
            Form::Text(text) => text.into_bytes(),
            Form::Bytes(bytes) => bytes,
        }
    }
}

pub fn serialize<S: Serializer>(name: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    match std::str::from_utf8(name) {
        Ok(text) => text.serialize(serializer),
        Err(_) => name.serialize(serializer),
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    Form::deserialize(deserializer).map(Vec::from)
}

pub mod option {
    use super::Form;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        name: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match name {
            Some(name) => serializer.serialize_some(&Name(name)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<Form>::deserialize(deserializer).map(|form| form.map(Vec::from))
    }

    struct Name<'a>(&'a [u8]);

    impl serde::Serialize for Name<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            super::serialize(self.0, serializer)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Named {
        #[serde(with = "super")]
        name: Vec<u8>,
        #[serde(with = "super::option", default)]
        alias: Option<Vec<u8>>,
    }

    #[test]
    fn a_utf8_name_is_a_string_and_other_bytes_stay_an_array() {
        let named = Named {
            name: b"pm_update_next_tbtt".to_vec(),
            alias: Some(vec![0xff, 0x41]),
        };
        let json = serde_json::to_string(&named).unwrap();
        assert_eq!(json, r#"{"name":"pm_update_next_tbtt","alias":[255,65]}"#);
        assert_eq!(serde_json::from_str::<Named>(&json).unwrap(), named);
        let none = Named {
            name: b"x".to_vec(),
            alias: None,
        };
        let json = serde_json::to_string(&none).unwrap();
        assert_eq!(json, r#"{"name":"x","alias":null}"#);
        assert_eq!(serde_json::from_str::<Named>(&json).unwrap(), none);
    }
}
