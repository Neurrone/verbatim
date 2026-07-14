//! Gesture identifiers.
//!
//! Verbatim follows NVDA's identifier scheme: `source:parts`, where the
//! source names the input kind (`kb` for keyboard; touch, braille, and
//! others join later) and the parts name keys or actions joined by plus
//! signs. Identifiers are normalized so binding lookups are order- and
//! case-insensitive: the whole identifier is lowercased and the parts are
//! sorted, exactly as NVDA's `normalizeGestureIdentifier` does — so
//! `kb:Verbatim+V` and `kb:v+verbatim` are the same gesture.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A normalized gesture identifier such as `kb:v+verbatim`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct GestureId {
    normalized: String,
}

impl GestureId {
    /// Parses and normalizes an identifier like `kb:verbatim+v`.
    ///
    /// # Errors
    ///
    /// Returns an error when the identifier has no `source:` prefix, an
    /// empty source, or empty parts.
    pub fn parse(raw: &str) -> Result<Self, GestureParseError> {
        let (source, keys) = raw
            .split_once(':')
            .ok_or_else(|| GestureParseError::MissingSource(raw.to_string()))?;
        if source.is_empty() {
            return Err(GestureParseError::MissingSource(raw.to_string()));
        }
        let mut parts: Vec<String> = keys.split('+').map(str::to_lowercase).collect();
        if parts.iter().any(String::is_empty) {
            return Err(GestureParseError::EmptyPart(raw.to_string()));
        }
        parts.sort_unstable();
        Ok(Self {
            normalized: format!("{}:{}", source.to_lowercase(), parts.join("+")),
        })
    }

    /// The input source prefix, such as `kb`.
    #[must_use]
    pub fn source(&self) -> &str {
        self.normalized
            .split_once(':')
            .map_or("", |(source, _)| source)
    }

    /// The full normalized identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.normalized
    }
}

impl fmt::Display for GestureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl FromStr for GestureId {
    type Err = GestureParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl<'de> Deserialize<'de> for GestureId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Error from [`GestureId::parse`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GestureParseError {
    /// The identifier has no `source:` prefix or an empty source.
    MissingSource(String),
    /// A plus-separated part is empty.
    EmptyPart(String),
}

impl fmt::Display for GestureParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSource(raw) => {
                write!(f, "gesture identifier {raw:?} has no source prefix")
            }
            Self::EmptyPart(raw) => {
                write!(f, "gesture identifier {raw:?} has an empty key part")
            }
        }
    }
}

impl std::error::Error for GestureParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_case_and_order_insensitive() {
        let one = GestureId::parse("kb:Verbatim+V").expect("parses");
        let two = GestureId::parse("kb:v+verbatim").expect("parses");
        assert_eq!(one, two);
        assert_eq!(one.as_str(), "kb:v+verbatim");
    }

    #[test]
    fn source_is_preserved() {
        let gesture = GestureId::parse("kb:tab").expect("parses");
        assert_eq!(gesture.source(), "kb");
    }

    #[test]
    fn missing_source_is_rejected() {
        assert!(matches!(
            GestureId::parse("verbatim+v"),
            Err(GestureParseError::MissingSource(_))
        ));
        assert!(matches!(
            GestureId::parse(":verbatim+v"),
            Err(GestureParseError::MissingSource(_))
        ));
    }

    #[test]
    fn empty_part_is_rejected() {
        assert!(matches!(
            GestureId::parse("kb:verbatim+"),
            Err(GestureParseError::EmptyPart(_))
        ));
    }

    #[test]
    fn serde_round_trips_through_normalization() {
        let gesture: GestureId = serde_json::from_str("\"kb:V+Verbatim\"").expect("deserializes");
        assert_eq!(gesture.as_str(), "kb:v+verbatim");
        assert_eq!(
            serde_json::to_string(&gesture).expect("serializes"),
            "\"kb:v+verbatim\""
        );
    }
}
