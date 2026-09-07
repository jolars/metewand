//! Versioned worker protocol types for Metewand.

pub mod framing;
mod messages;

pub use messages::{
    Capability, HandshakeError, HelloRequest, HelloResponse, MessageDecodeError,
    MessageEncodeError, MetadataError, NegotiatedSession, RequestId, RequestTracker,
    RequestTrackerError, SdkMetadata, WireMessage, WorkerIdentity, WorkerRole, decode_message,
    encode_message, negotiate_protocol, validate_hello_response,
};

/// Version of the JSON-Lines worker protocol.
pub const WIRE_PROTOCOL_VERSION: u32 = 1;

/// Repository path of the version-1 worker message schema.
pub const WORKER_MESSAGE_SCHEMA_PATH: &str = "schemas/v1/worker-protocol-message.schema.json";

/// Stable identifier of the version-1 worker message schema.
pub const WORKER_MESSAGE_SCHEMA_ID: &str =
    "metewand://schemas/v1/worker-protocol-message.schema.json";

/// Exact checked-in version-1 worker message schema.
pub const WORKER_MESSAGE_SCHEMA: &str =
    include_str!("../../../schemas/v1/worker-protocol-message.schema.json");

#[cfg(test)]
mod tests {
    use super::WIRE_PROTOCOL_VERSION;

    #[test]
    fn wire_protocol_version_starts_at_one() {
        assert_eq!(WIRE_PROTOCOL_VERSION, 1);
    }
}
