//! Secret values (spec section 7.3).
//!
//! These types replace the generated `Secrets` type so that passwords are
//! zeroized on drop and can never be printed through `Debug`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

/// A password or other secret string. Zeroized on drop; `Debug` prints `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// The secret value. Only call this to hand it to a worker or an engine.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

/// The `secrets` field of a core message. Mirrors `#/definitions/Secrets`;
/// schema validation in [`crate::Codec`] enforces its limits.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<SecretString>,
    #[serde(rename = "userPassword", skip_serializing_if = "Option::is_none")]
    pub user_password: Option<SecretString>,
    #[serde(rename = "ownerPassword", skip_serializing_if = "Option::is_none")]
    pub owner_password: Option<SecretString>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets { .. }")
    }
}
