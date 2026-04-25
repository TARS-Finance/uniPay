use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(#[serde(with = "time::serde::rfc3339")] pub OffsetDateTime);

impl Default for Timestamp {
    fn default() -> Self {
        Self(OffsetDateTime::UNIX_EPOCH)
    }
}

impl Timestamp {
    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
