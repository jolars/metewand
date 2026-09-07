use std::str::FromStr;

use metewand_core::{
    EXECUTION_SEMANTICS_VERSION, IDENTITY_VERSION, MANIFEST_VERSION, PUBLIC_SCHEMA_VERSION,
    SCHEDULING_POLICY_VERSION, SEED_DERIVATION_VERSION,
    public_schemas::{PublicSchema, public_schema_catalog},
};
use metewand_protocol::{
    Capability, HelloRequest, HelloResponse, RequestId, SdkMetadata, WIRE_PROTOCOL_VERSION,
    WORKER_MESSAGE_SCHEMA, WORKER_MESSAGE_SCHEMA_ID, WorkerIdentity, WorkerRole,
};
use metewand_runtime::LOCAL_TREE_HASH_VERSION;

#[test]
fn workspace_exposes_initial_compatibility_versions() {
    assert_eq!(MANIFEST_VERSION, 1);
    assert_eq!(PUBLIC_SCHEMA_VERSION, 1);
    assert_eq!(WIRE_PROTOCOL_VERSION, 1);
    assert_eq!(IDENTITY_VERSION, 1);
    assert_eq!(SEED_DERIVATION_VERSION, 1);
    assert_eq!(SCHEDULING_POLICY_VERSION, 1);
    assert_eq!(EXECUTION_SEMANTICS_VERSION, 1);
    assert_eq!(LOCAL_TREE_HASH_VERSION, 1);
}

#[test]
fn typed_worker_messages_match_the_public_wire_schema() {
    let worker_id = WorkerIdentity::from_str(
        "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .expect("worker identity must be valid");
    let request = HelloRequest::new(
        RequestId::from_str("0").expect("request ID must be valid"),
        WorkerRole::Implementation,
        worker_id.clone(),
    );
    let response = HelloResponse::new(
        request.id().clone(),
        worker_id,
        Some(SdkMetadata::new("metewand-rust", "0.1.0").expect("SDK metadata must be valid")),
        [Capability::OneShot],
    );

    assert_eq!(
        PublicSchema::WorkerProtocolMessage.id(),
        WORKER_MESSAGE_SCHEMA_ID
    );
    assert_eq!(
        PublicSchema::WorkerProtocolMessage.source(),
        WORKER_MESSAGE_SCHEMA
    );
    let catalog = public_schema_catalog().expect("public schemas must compile");
    for message in [
        serde_json::to_value(request).expect("request must serialize"),
        serde_json::to_value(response).expect("response must serialize"),
    ] {
        catalog
            .validate(
                PublicSchema::WorkerProtocolMessage.repository_path(),
                &message,
            )
            .expect("typed message must satisfy the public wire schema");
    }
}
