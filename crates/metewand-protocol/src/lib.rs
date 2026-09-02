//! Versioned worker protocol types for Metewand.

/// Version of the JSON-Lines worker protocol.
pub const WIRE_PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::WIRE_PROTOCOL_VERSION;

    #[test]
    fn wire_protocol_version_starts_at_one() {
        assert_eq!(WIRE_PROTOCOL_VERSION, 1);
    }
}
