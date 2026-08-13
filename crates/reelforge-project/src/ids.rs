//! Opaque project / timeline ids (not vision query primitives).

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! pid {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);
        impl $name {
            /// Construct.
            #[must_use]
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }
            /// Borrow.
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
    };
}

pid!(
    /// Project document id.
    ProjectId
);
pid!(
    /// Sequence id.
    SequenceId
);
pid!(
    /// Timeline track id.
    TimelineTrackId
);
pid!(
    /// Timeline clip id (not [`reelforge_core::ClipId`]).
    TimelineClipId
);
pid!(
    /// Media library entry id.
    MediaRefId
);
