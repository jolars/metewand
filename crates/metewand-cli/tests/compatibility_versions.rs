use metewand_core::{
    EXECUTION_SEMANTICS_VERSION, IDENTITY_VERSION, MANIFEST_VERSION, PUBLIC_SCHEMA_VERSION,
};
use metewand_protocol::WIRE_PROTOCOL_VERSION;

#[test]
fn workspace_exposes_initial_compatibility_versions() {
    assert_eq!(MANIFEST_VERSION, 1);
    assert_eq!(PUBLIC_SCHEMA_VERSION, 1);
    assert_eq!(WIRE_PROTOCOL_VERSION, 1);
    assert_eq!(IDENTITY_VERSION, 1);
    assert_eq!(EXECUTION_SEMANTICS_VERSION, 1);
}
