//! The global OpenAPI revalidation interval, converted from integer seconds when parsed.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiConfig {
    /// Zero revalidates on every load; the default reuses a copy for one hour.
    #[serde(
        default = "default_interval",
        deserialize_with = "deserialize_interval",
        serialize_with = "serialize_interval"
    )]
    pub revalidate_after: Duration,
}

impl Default for OpenApiConfig {
    fn default() -> Self {
        Self {
            revalidate_after: default_interval(),
        }
    }
}

fn default_interval() -> Duration {
    Duration::from_secs(3600)
}

fn deserialize_interval<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    let seconds = u64::deserialize(deserializer).map_err(|error| {
        serde::de::Error::custom(format!(
            "[openapi] revalidate_after must be a non-negative integer number of seconds: {error}"
        ))
    })?;
    Ok(Duration::from_secs(seconds))
}

fn serialize_interval<S: serde::Serializer>(
    value: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(value.as_secs())
}
