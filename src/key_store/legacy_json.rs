//! Strict parsing only for explicit legacy migration, never a runtime fallback.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

struct Document(Value);
impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        struct Strict;
        impl<'de> Visitor<'de> for Strict {
            type Value = Document;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unambiguous legacy JSON")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut input: M) -> Result<Document, M::Error> {
                let mut values = Map::new();
                while let Some((name, Document(value))) = input.next_entry::<String, Document>()? {
                    if values.insert(name, value).is_some() {
                        return Err(de::Error::custom("duplicate legacy field"));
                    }
                }
                Ok(Document(Value::Object(values)))
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut input: S) -> Result<Document, S::Error> {
                let mut values = Vec::new();
                while let Some(Document(value)) = input.next_element()? {
                    values.push(value);
                }
                Ok(Document(Value::Array(values)))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Document, E> {
                Ok(Document(Value::String(value.to_owned())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Document, E> {
                Ok(Document(Value::String(value)))
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Document, E> {
                Ok(Document(Value::Bool(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Document, E> {
                Ok(Document(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Document, E> {
                Ok(Document(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Document, E> {
                serde_json::Number::from_f64(value)
                    .map(|value| Document(Value::Number(value)))
                    .ok_or_else(|| de::Error::custom("invalid number"))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Document, E> {
                Ok(Document(Value::Null))
            }
        }
        input.deserialize_any(Strict)
    }
}

pub(crate) fn parse(bytes: &[u8]) -> super::Result<Value> {
    serde_json::from_slice::<Document>(bytes)
        .map(|value| value.0)
        .map_err(|_| super::Error::Invalid)
}
