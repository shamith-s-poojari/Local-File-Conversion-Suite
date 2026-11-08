//! Worker protocol (spec section 7).
//!
//! The types in [`types`] are generated at compile time from
//! `packages/protocol/schema/protocol.schema.json`, so they can't drift from
//! the schema. The generated types don't enforce every schema rule (constants,
//! numeric ranges), so [`Codec`] validates each line against the schema itself
//! before deserializing it. Always decode through [`Codec`].

mod codec;
mod secret;

pub use codec::{Codec, DecodeError, Direction};
pub use secret::{SecretString, Secrets};

/// Generated protocol types.
#[allow(clippy::all, clippy::pedantic, missing_docs)]
pub mod types {
    typify::import_types!(
        schema = "../../packages/protocol/schema/protocol.schema.json",
        replace = {
            Secrets = crate::Secrets,
        },
    );
}

pub use types::{CoreMessage, WorkerMessage};

/// Protocol version. Must equal `#/definitions/ProtocolVersion` in the schema.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum length of one NDJSON line in bytes, excluding the newline (spec section 7.1).
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// The protocol schema, embedded at compile time.
pub const SCHEMA_JSON: &str =
    include_str!("../../../packages/protocol/schema/protocol.schema.json");
