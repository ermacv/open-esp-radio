//! The optional wire forms of [`Duration`]: transparently its `u64`
//! microseconds (feature `serde`), and that same shape as a
//! `postcard_schema::Schema` (feature `schema`).

use crate::Duration;

#[cfg(feature = "serde")]
impl serde::Serialize for Duration {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.as_micros())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Duration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <u64 as serde::Deserialize>::deserialize(deserializer).map(Self::from_micros)
    }
}

#[cfg(feature = "schema")]
impl postcard_schema::Schema for Duration {
    const SCHEMA: &'static postcard_schema::schema::NamedType =
        <u64 as postcard_schema::Schema>::SCHEMA;
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn a_duration_round_trips_through_postcard_as_its_microseconds() {
        let duration = Duration::from_micros(1_250);
        let bytes = postcard::to_allocvec(&duration).unwrap();
        assert_eq!(bytes, postcard::to_allocvec(&1_250_u64).unwrap());
        assert_eq!(postcard::from_bytes::<Duration>(&bytes).unwrap(), duration);
        let largest = Duration::from_micros(u64::MAX);
        assert_eq!(
            postcard::from_bytes::<Duration>(&postcard::to_allocvec(&largest).unwrap()).unwrap(),
            largest
        );
    }

    #[cfg(feature = "schema")]
    #[test]
    fn its_schema_is_the_u64_it_is_written_as() {
        use postcard_schema::Schema;
        assert_eq!(Duration::SCHEMA, u64::SCHEMA);
    }
}
