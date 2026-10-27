//! Core domain types, planning, scheduling, and run records for Metewand.

pub mod applicability;
pub mod cache;
pub mod canonical;
pub mod comparison;
pub mod compatibility;
pub mod controls;
pub mod export;
mod field_declaration;
pub mod identity;
pub mod inspection;
pub mod manifest;
pub mod manifest_hash;
pub mod materialization;
pub mod measurements;
mod parameter_source;
pub mod parameters;
pub mod planning;
pub mod problem_contract;
pub mod profile;
pub mod public_schemas;
pub mod records;
pub mod run_documents;
pub mod run_output;
pub mod scheduling;
pub mod schema;
pub mod seed;
mod yaml;

/// Version of the typed Metewand manifest format.
pub const MANIFEST_VERSION: u32 = 1;

/// Version shared by Metewand-owned public schemas.
pub const PUBLIC_SCHEMA_VERSION: u32 = 1;

/// Version of the typed content-identity construction.
pub const IDENTITY_VERSION: u32 = 1;

/// Version of the deterministic component-seed derivation transcript.
pub const SEED_DERIVATION_VERSION: u32 = 1;

/// Version of the deterministic scheduling policy bound into scheduling seeds.
pub const SCHEDULING_POLICY_VERSION: u32 = 1;

/// Version of execution behavior that contributes to resolved identities.
pub const EXECUTION_SEMANTICS_VERSION: u32 = 2;

#[cfg(test)]
mod tests {
    use super::{
        EXECUTION_SEMANTICS_VERSION, IDENTITY_VERSION, MANIFEST_VERSION, PUBLIC_SCHEMA_VERSION,
        SCHEDULING_POLICY_VERSION, SEED_DERIVATION_VERSION,
    };

    #[test]
    fn compatibility_versions_bind_the_current_contracts() {
        assert_eq!(MANIFEST_VERSION, 1);
        assert_eq!(PUBLIC_SCHEMA_VERSION, 1);
        assert_eq!(IDENTITY_VERSION, 1);
        assert_eq!(SEED_DERIVATION_VERSION, 1);
        assert_eq!(SCHEDULING_POLICY_VERSION, 1);
        assert_eq!(EXECUTION_SEMANTICS_VERSION, 2);
        assert_eq!(crate::export::EXPORT_SCHEMA_VERSION, 1);
        assert_eq!(crate::cache::CACHE_MAINTENANCE_VERSION, 1);
    }
}
