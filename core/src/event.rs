//! Attributed activity events and their Swift-compatible JSON encoding.
//!
//! Swift's synthesized `Codable` writes payload-less enum cases as
//! `{"created":{}}`, `URL`s as `file://` strings (directories with a trailing
//! slash), and `Date`s as whole-second ISO 8601. Everything here exists to
//! reproduce that shape exactly.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EventKind {
    Created,
    Modified,
    Deleted,
    Moved,
    Aggregate,
}

impl EventKind {
    const CASES: [(EventKind, &'static str); 5] = [
        (EventKind::Created, "created"),
        (EventKind::Modified, "modified"),
        (EventKind::Deleted, "deleted"),
        (EventKind::Moved, "moved"),
        (EventKind::Aggregate, "aggregate"),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Confidence {
    Confirmed,
    Estimated,
    Unknown,
}

impl Confidence {
    const CASES: [(Confidence, &'static str); 3] = [
        (Confidence::Confirmed, "confirmed"),
        (Confidence::Estimated, "estimated"),
        (Confidence::Unknown, "unknown"),
    ];
}

/// One recorded change with its byte delta. Paths are plain filesystem paths;
/// the JSON form converts them to `file://` URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    #[serde(with = "swift_case::event_kind")]
    pub kind: EventKind,
    #[serde(with = "file_url::path")]
    pub path: String,
    #[serde(with = "file_url::dir")]
    pub root_path: String,
    #[serde(with = "swift_date")]
    pub timestamp: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_delta: Option<i64>,
    #[serde(with = "swift_case::confidence")]
    pub confidence: Confidence,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "file_url::opt_path"
    )]
    pub previous_path: Option<String>,
    pub affected_item_count: u32,
}

impl ActivityEvent {
    /// Aggregate rows point at the watch root itself, which Swift writes as a
    /// directory URL. Fixing this up after (de)serialization keeps the field
    /// serde modules simple.
    fn normalize_aggregate_path(mut self) -> Self {
        if self.kind == EventKind::Aggregate && !self.path.ends_with('/') && self.path.len() > 1 {
            self.path.push('/');
        }
        self
    }

    pub fn to_json_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(&self.clone().normalize_aggregate_path())
    }

    pub fn from_json_line(line: &str) -> serde_json::Result<Self> {
        let event: ActivityEvent = serde_json::from_str(line)?;
        Ok(event.strip_aggregate_slash())
    }

    fn strip_aggregate_slash(mut self) -> Self {
        if self.path.len() > 1 {
            while self.path.ends_with('/') && self.path.len() > 1 {
                self.path.pop();
            }
        }
        self
    }
}

/// `{"caseName":{}}` — Swift's externally tagged form for enums without payloads.
mod swift_case {
    use std::collections::BTreeMap;

    use serde::de::Error as _;
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serializer};

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Empty {}

    pub fn serialize_case<S: Serializer>(name: &str, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(name, &Empty {})?;
        map.end()
    }

    pub fn deserialize_case<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<String, D::Error> {
        let map: BTreeMap<String, Empty> = BTreeMap::deserialize(deserializer)?;
        map.into_keys()
            .next()
            .ok_or_else(|| D::Error::custom("enum case object is empty"))
    }

    macro_rules! case_module {
        ($module:ident, $type:ty) => {
            pub mod $module {
                use serde::de::Error as _;
                use serde::{Deserializer, Serializer};

                pub fn serialize<S: Serializer>(
                    value: &$type,
                    serializer: S,
                ) -> Result<S::Ok, S::Error> {
                    let name = <$type>::CASES
                        .iter()
                        .find(|(case, _)| case == value)
                        .map(|(_, name)| *name)
                        .expect("every case is listed");
                    super::serialize_case(name, serializer)
                }

                pub fn deserialize<'de, D: Deserializer<'de>>(
                    deserializer: D,
                ) -> Result<$type, D::Error> {
                    let name = super::deserialize_case(deserializer)?;
                    <$type>::CASES
                        .iter()
                        .find(|(_, candidate)| *candidate == name)
                        .map(|(case, _)| *case)
                        .ok_or_else(|| D::Error::custom(format!("unknown case {name:?}")))
                }
            }
        };
    }

    case_module!(event_kind, crate::event::EventKind);
    case_module!(confidence, crate::event::Confidence);
}

/// Whole-second RFC 3339 in UTC, matching `JSONEncoder.dateEncodingStrategy = .iso8601`.
mod swift_date {
    use std::time::SystemTime;

    use serde::de::Error as _;
    use serde::ser::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};
    use time::format_description::well_known::Rfc3339;
    use time::{OffsetDateTime, UtcOffset};

    pub fn serialize<S: Serializer>(value: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
        let datetime = OffsetDateTime::from(*value)
            .to_offset(UtcOffset::UTC)
            .replace_nanosecond(0)
            .map_err(S::Error::custom)?;
        serializer.serialize_str(&datetime.format(&Rfc3339).map_err(S::Error::custom)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        OffsetDateTime::parse(&text, &Rfc3339)
            .map(SystemTime::from)
            .map_err(D::Error::custom)
    }
}

/// `file://` URL <-> filesystem path.
// ponytail: POSIX paths only; Windows drive letters need `file:///C:/...` handling.
pub mod file_url {
    use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer};

    const PATH_SET: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'[')
        .add(b'\\')
        .add(b']')
        .add(b'^')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}');

    pub fn encode(path: &str, is_directory: bool) -> String {
        let mut url = format!("file://{}", utf8_percent_encode(path, PATH_SET));
        if is_directory && !url.ends_with('/') {
            url.push('/');
        }
        url
    }

    pub fn decode(url: &str) -> Option<String> {
        let rest = url.strip_prefix("file://")?;
        let decoded = percent_decode_str(rest).decode_utf8().ok()?.into_owned();
        Some(decoded)
    }

    fn deserialize_path<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        let url = String::deserialize(deserializer)?;
        decode(&url).ok_or_else(|| D::Error::custom(format!("not a file URL: {url:?}")))
    }

    fn strip_trailing_slash(mut path: String) -> String {
        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        path
    }

    pub mod path {
        use serde::{Deserializer, Serializer};

        pub fn serialize<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&super::encode(value, value.ends_with('/')))
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
            super::deserialize_path(deserializer)
        }
    }

    pub mod dir {
        use serde::{Deserializer, Serializer};

        pub fn serialize<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&super::encode(value, true))
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
            super::deserialize_path(deserializer).map(super::strip_trailing_slash)
        }
    }

    pub mod opt_path {
        use serde::{Deserializer, Serializer};

        pub fn serialize<S: Serializer>(
            value: &Option<String>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                Some(path) => super::path::serialize(path, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<String>, D::Error> {
            super::deserialize_path(deserializer).map(Some)
        }
    }
}
