use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($(#[$m:meta])* $n:ident) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $n(String);
        impl $n {
            pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl fmt::Display for $n {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
        }
        impl From<&str> for $n { fn from(s: &str) -> Self { Self::new(s) } }
    };
}

id!(/// A kind of game (identity is a string, never an enum).
    SystemId);
id!(/// Something that starts games.
    RunnerId);
id!(/// A catalog or metadata provider.
    ProviderId);
id!(/// A game's key, scoped to its system.
    GameKey);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GameRef {
    pub system: SystemId,
    pub key: GameKey,
}
