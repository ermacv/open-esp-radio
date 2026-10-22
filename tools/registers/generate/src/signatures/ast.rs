//! Clang's JSON locations omit a repeated filename. Preserve the dump's
//! member order until every location has its actual file; includedFrom is
//! an include parent and does not change that source-file cursor.
use crate::Result;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fmt;

#[derive(Debug)]
enum Ordered {
    Scalar(Value),
    Object(Vec<(String, Ordered)>),
    Array(Vec<Ordered>),
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Read;
        impl<'de> Visitor<'de> for Read {
            type Value = Ordered;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Clang JSON")
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(v.into()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Ordered, E> {
                Ok(Ordered::Scalar(v.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Ordered, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = a.next_element()? {
                    values.push(v);
                }
                Ok(Ordered::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Ordered, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = a.next_entry()? {
                    values.push(v);
                }
                Ok(Ordered::Object(values))
            }
        }
        d.deserialize_any(Read)
    }
}

impl Ordered {
    fn resolve(self, last_file: &mut Option<String>) -> Value {
        match self {
            Self::Scalar(v) => v,
            Self::Array(v) => Value::Array(v.into_iter().map(|v| v.resolve(last_file)).collect()),
            Self::Object(v) => {
                let location =
                    v.iter().any(|(k, _)| k == "offset") && v.iter().any(|(k, _)| k == "col");
                if location
                    && let Some(file) = v.iter().find_map(|(k, v)| match (k.as_str(), v) {
                        ("file", Self::Scalar(Value::String(s))) => Some(s.clone()),
                        _ => None,
                    })
                {
                    *last_file = Some(file);
                }
                let actual_file = location.then(|| last_file.clone()).flatten();
                let mut out: serde_json::Map<_, _> = v
                    .into_iter()
                    .map(|(k, v)| (k, v.resolve(last_file)))
                    .collect();
                if let Some(file) = actual_file {
                    out.insert("actualFile".into(), file.into());
                }
                Value::Object(out)
            }
        }
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Value> {
    Ok(serde_json::from_slice::<Ordered>(bytes)?.resolve(&mut None))
}

pub(super) fn nodes<'a>(root: &'a Value, kind: &str, out: &mut Vec<&'a Value>) {
    if root.get("kind").and_then(Value::as_str) == Some(kind) {
        out.push(root);
    }
    if let Some(inner) = root.get("inner").and_then(Value::as_array) {
        for child in inner {
            nodes(child, kind, out);
        }
    }
}

pub(super) fn children(root: &Value) -> &[Value] {
    root.get("inner")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
