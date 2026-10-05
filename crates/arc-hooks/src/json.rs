use std::fmt;

use indexmap::IndexMap;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Number;

pub type Object = IndexMap<String, Json>;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Json {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Json>),
    Object(Object),
}

impl Json {
    pub fn object<const N: usize>(pairs: [(&str, Json); N]) -> Self {
        Json::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    pub fn empty_object() -> Self {
        Json::Object(Object::new())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(map) => map.get_mut(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Json::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Json::Array(_))
    }

    pub fn take(&mut self) -> Json {
        std::mem::take(self)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Self {
        Json::String(s.to_owned())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Self {
        Json::String(s)
    }
}

impl From<u32> for Json {
    fn from(n: u32) -> Self {
        Json::Number(n.into())
    }
}

impl From<Vec<Json>> for Json {
    fn from(items: Vec<Json>) -> Self {
        Json::Array(items)
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => serializer.serialize_unit(),
            Json::Bool(b) => serializer.serialize_bool(*b),
            Json::Number(n) => n.serialize(serializer),
            Json::String(s) => serializer.serialize_str(s),
            Json::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                items
                    .iter()
                    .try_for_each(|item| seq.serialize_element(item))?;
                seq.end()
            }
            Json::Object(map) => {
                let mut out = serializer.serialize_map(Some(map.len()))?;
                map.iter()
                    .try_for_each(|(k, v)| out.serialize_entry(k, v))?;
                out.end()
            }
        }
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
        Ok(Json::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
        Ok(Json::Number(n.into()))
    }

    fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
        Ok(Json::Number(n.into()))
    }

    fn visit_f64<E: de::Error>(self, n: f64) -> Result<Json, E> {
        Number::from_f64(n)
            .map(Json::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E>(self, s: &str) -> Result<Json, E> {
        Ok(Json::String(s.to_owned()))
    }

    fn visit_string<E>(self, s: String) -> Result<Json, E> {
        Ok(Json::String(s))
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
        Json::deserialize(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Json, A::Error> {
        let mut map = Object::new();
        while let Some((k, v)) = access.next_entry::<String, Json>()? {
            map.insert(k, v);
        }
        Ok(Json::Object(map))
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Json, D::Error> {
        d.deserialize_any(JsonVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_keeping_key_order() {
        let text = r#"{"z":1,"a":{"y":[true,null,-2,1.5,"s"],"b":{}},"m":[]}"#;
        let json: Json = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&json).unwrap(), text);
        let Json::Object(map) = &json else {
            panic!("{json:?}")
        };
        assert_eq!(map.keys().collect::<Vec<_>>(), ["z", "a", "m"]);
        assert!(serde_json::from_str::<Json>("{").is_err());
    }
}
