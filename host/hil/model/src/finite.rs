//! A serializer that only checks that a value holds no NaN or infinite
//! number. JSON has no form for them (`serde_json` writes `null`), so two
//! different non-finite thresholds would otherwise share one identity.

use core::fmt;

use serde::{Serialize, ser};

/// A value holds a NaN or an infinite number.
#[derive(Debug)]
pub struct NonFinite(String);

impl fmt::Display for NonFinite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NonFinite {}

impl ser::Error for NonFinite {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// Fails when `value` serializes a NaN or an infinite number.
pub fn check<T: Serialize + ?Sized>(value: &T) -> Result<(), NonFinite> {
    value.serialize(Finite)
}

struct Finite;

fn float(value: f64) -> Result<(), NonFinite> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(NonFinite(format!("{value} has no JSON form")))
    }
}

macro_rules! accept {
    ($($method:ident($type:ty);)*) => {$(
        fn $method(self, _: $type) -> Result<(), NonFinite> {
            Ok(())
        }
    )*};
}

macro_rules! compound {
    ($($trait:ident { $($method:ident $(<$key:ident>)? ($($arg:ident: $argty:ty),*);)* })*) => {$(
        impl ser::$trait for Finite {
            type Ok = ();
            type Error = NonFinite;
            $(
                fn $method<T: Serialize + ?Sized>(&mut self, $($arg: $argty,)* value: &T) -> Result<(), NonFinite> {
                    $(let _ = $arg;)*
                    value.serialize(Finite)
                }
            )*
            fn end(self) -> Result<(), NonFinite> {
                Ok(())
            }
        }
    )*};
}

compound! {
    SerializeSeq { serialize_element(); }
    SerializeTuple { serialize_element(); }
    SerializeTupleStruct { serialize_field(); }
    SerializeTupleVariant { serialize_field(); }
    SerializeStruct { serialize_field(key: &'static str); }
    SerializeStructVariant { serialize_field(key: &'static str); }
}

impl ser::SerializeMap for Finite {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), NonFinite> {
        key.serialize(Finite)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NonFinite> {
        value.serialize(Finite)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl ser::Serializer for Finite {
    type Ok = ();
    type Error = NonFinite;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    accept! {
        serialize_bool(bool);
        serialize_i8(i8);
        serialize_i16(i16);
        serialize_i32(i32);
        serialize_i64(i64);
        serialize_i128(i128);
        serialize_u8(u8);
        serialize_u16(u16);
        serialize_u32(u32);
        serialize_u64(u64);
        serialize_u128(u128);
        serialize_char(char);
        serialize_str(&str);
        serialize_bytes(&[u8]);
        serialize_unit_struct(&'static str);
    }

    fn serialize_f32(self, value: f32) -> Result<(), NonFinite> {
        float(f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<(), NonFinite> {
        float(value)
    }

    fn serialize_none(self) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), NonFinite> {
        value.serialize(Finite)
    }

    fn serialize_unit(self) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), NonFinite> {
        value.serialize(Finite)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), NonFinite> {
        value.serialize(Finite)
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self, NonFinite> {
        Ok(Self)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self, NonFinite> {
        Ok(Self)
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, NonFinite> {
        Ok(Self)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, NonFinite> {
        Ok(Self)
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self, NonFinite> {
        Ok(Self)
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, NonFinite> {
        Ok(Self)
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, NonFinite> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn non_finite_numbers_are_found_anywhere() {
        assert!(check(&1.5_f32).is_ok());
        assert!(check(&f32::NAN).is_err());
        let nested: BTreeMap<String, Vec<Option<(u8, f64)>>> =
            BTreeMap::from([("a".to_owned(), vec![None, Some((1, f64::INFINITY))])]);
        assert!(check(&nested).is_err());
        let finite: BTreeMap<String, Vec<Option<(u8, f64)>>> =
            BTreeMap::from([("a".to_owned(), vec![None, Some((1, 2.0))])]);
        assert!(check(&finite).is_ok());
    }
}
