use crate::core::{
    IDENTITY_VERSION,
    canonical::CanonicalValue,
    identity::canonical_resource_identity_bytes,
    inspection::{diagnostic_id, is_inspection_id},
};
use serde_json::{Value, json};

#[test]
fn diagnostic_identities_match_exact_version_one_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../fixtures/identities/diagnostics-v1.json"
    ))
    .unwrap();
    assert_eq!(vectors["version"], IDENTITY_VERSION);
    for vector in vectors["vectors"].as_array().unwrap() {
        let diagnostic = &vector["diagnostic"];
        let canonical = CanonicalValue::try_from(json!({
            "code": diagnostic["code"], "message": diagnostic["message"], "details": diagnostic.get("details").unwrap_or(&Value::Null),
        })).unwrap();
        assert_eq!(
            String::from_utf8(canonical_resource_identity_bytes(&canonical).unwrap()).unwrap(),
            vector["canonical"]
        );
        let id = diagnostic_id(diagnostic).unwrap();
        assert_eq!(id.to_string(), vector["id"]);
        let mut reordered = diagnostic.clone();
        if reordered.get("details").is_none() {
            reordered["details"] = Value::Null;
        }
        reordered["attempt_id"] = json!("provenance does not identify a diagnostic");
        assert_eq!(diagnostic_id(&reordered).unwrap(), id);
        reordered["message"] = json!("Another failure.");
        assert_ne!(diagnostic_id(&reordered).unwrap(), id);
    }
}

#[test]
fn inspection_requires_complete_canonical_identities() {
    let id = format!("mw1-attempt-{}", "a".repeat(64));
    for valid in [
        &id,
        &format!("run-store-v1:attempt-{}", "a".repeat(64)),
        &format!(
            "run-store-v1:unconditional:mw1-logical-candidate-{}",
            "a".repeat(64)
        ),
    ] {
        assert!(is_inspection_id(valid));
    }
    for invalid in [
        "attempt",
        "mw1-attempt-abc",
        &format!("mw2-attempt-{}", "a".repeat(64)),
        &id.to_uppercase(),
        &format!("{id}/../file"),
    ] {
        assert!(!is_inspection_id(invalid));
    }
}
