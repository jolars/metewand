//! Core domain types, planning, scheduling, and run records for Metewand.

pub mod canonical;
pub mod parameters;
pub mod public_schemas;
pub mod schema;

/// Version of the typed Metewand manifest format.
pub const MANIFEST_VERSION: u32 = 1;

/// Version shared by Metewand-owned public schemas.
pub const PUBLIC_SCHEMA_VERSION: u32 = 1;

/// Version of the typed content-identity construction.
pub const IDENTITY_VERSION: u32 = 1;

/// Version of execution behavior that contributes to resolved identities.
pub const EXECUTION_SEMANTICS_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::{
        EXECUTION_SEMANTICS_VERSION, IDENTITY_VERSION, MANIFEST_VERSION, PUBLIC_SCHEMA_VERSION,
    };

    #[test]
    fn compatibility_versions_start_at_one() {
        assert_eq!(MANIFEST_VERSION, 1);
        assert_eq!(PUBLIC_SCHEMA_VERSION, 1);
        assert_eq!(IDENTITY_VERSION, 1);
        assert_eq!(EXECUTION_SEMANTICS_VERSION, 1);
    }
}
