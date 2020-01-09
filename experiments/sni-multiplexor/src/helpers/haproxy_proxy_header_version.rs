use std::fmt;

use serde::ser::Serializer;
use serde::de::{Visitor, Deserializer, Unexpected};

use crate::model::HaproxyProxyHeaderVersion; 

const HAPROXY_USE_VERSION_1: &str = "use-haproxy-v1";
const HAPROXY_USE_VERSION_2: &str = "use-haproxy-v2";
const HAPROXY_USE_NONE: &str = "none";

pub fn serialize<S>(value: &Option<HaproxyProxyHeaderVersion>, serializer: S) -> Result<S::Ok, S::Error>
where S: Serializer,
{
    serializer.serialize_str(match *value {
        Some(HaproxyProxyHeaderVersion::Version1) => HAPROXY_USE_VERSION_1,
        Some(HaproxyProxyHeaderVersion::Version2) => HAPROXY_USE_VERSION_2,
        None => HAPROXY_USE_NONE,
    })
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<HaproxyProxyHeaderVersion>, D::Error>
where
    D: Deserializer<'de>
{
    deserializer.deserialize_str(_HaproxyProxyHeaderVersion)
}

struct _HaproxyProxyHeaderVersion;

impl<'de> Visitor<'de> for _HaproxyProxyHeaderVersion {
    type Value = Option<HaproxyProxyHeaderVersion>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a string in {{{:?}, {:?}. {:?}}}",
            HAPROXY_USE_VERSION_1,
            HAPROXY_USE_VERSION_2,
            HAPROXY_USE_NONE)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error
    {
        match v {
            HAPROXY_USE_VERSION_1 => Ok(Some(HaproxyProxyHeaderVersion::Version1)),
            HAPROXY_USE_VERSION_2 => Ok(Some(HaproxyProxyHeaderVersion::Version2)),
            HAPROXY_USE_NONE => Ok(None),
            _ => Err(E::invalid_value(Unexpected::Str(v), &self))
        }
    }
}
