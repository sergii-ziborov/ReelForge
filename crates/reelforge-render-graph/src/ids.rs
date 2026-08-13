//! Opaque identity newtypes for tracks (not vision-query primitives).
//!
//! These ids are **handles**. `ReelForge` does not rank subjects, count visits,
//! or resolve re-id — that stays in `SightLoom` / Intelligence.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! opaque_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);

        impl $name {
            /// Construct from any string-like id.
            #[must_use]
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// Borrow the raw id.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

opaque_id!(
    /// Stable subject handle within a graph (person, plate, face, …).
    ///
    /// Not a query primitive: no visit counts, no “most frequent”.
    SubjectId
);

opaque_id!(
    /// One tracker trajectory (`SightLoom` `TrackId` / adapter fill).
    TrackId
);

opaque_id!(
    /// One appearance / visit segment on a track.
    AppearanceId
);

opaque_id!(
    /// One detector observation that produced a sample.
    ObservationId
);

impl SubjectId {
    /// Anonymous single-track identity when samples omit `subject`.
    #[must_use]
    pub fn anonymous() -> Self {
        Self("_anon".into())
    }

    /// Whether this is the anonymous bucket.
    #[must_use]
    pub fn is_anonymous(&self) -> bool {
        self.0 == "_anon"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_types() {
        let s = SubjectId::new("a");
        let t = TrackId::new("a");
        assert_eq!(s.as_str(), t.as_str());
        assert!(SubjectId::anonymous().is_anonymous());
    }
}
