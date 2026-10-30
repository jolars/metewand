use std::{collections::BTreeMap, time::Duration};

use rusqlite::{Connection, OpenFlags, params};

use super::{ArtifactStore, ArtifactStoreError, missing, regular_file};

/// The cache catalog has a separate layout version from content manifests.
pub const CACHE_CATALOG_VERSION: u32 = 1;
const NAME: &str = "cache.sqlite3";
const SCHEMA: &str = include_str!("catalog.sql");

pub(super) struct Catalog(pub Connection);

#[derive(Default)]
pub(super) struct Snapshot {
    pub objects: BTreeMap<String, String>,
    pub roots: BTreeMap<String, Vec<String>>,
    pub pending: BTreeMap<String, bool>,
}

// Every caller holds the cache lock, including across a snapshot and collection.
impl Catalog {
    pub fn initialize(store: &ArtifactStore) -> Result<Self, ArtifactStoreError> {
        Self::connect(store, true)
    }

    pub fn open(store: &ArtifactStore) -> Result<Self, ArtifactStoreError> {
        Self::connect(store, false)
    }

    fn connect(store: &ArtifactStore, initialize: bool) -> Result<Self, ArtifactStoreError> {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let path = store.root.join(format!("{NAME}{suffix}"));
            if !missing(&path)? {
                regular_file(&path)?;
            }
        }
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        if initialize {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let mut connection = Connection::open_with_flags(store.root.join(NAME), flags)?;
        #[cfg(test)]
        crate::runtime::fault_injection::trace_database(&connection);
        connection.busy_timeout(Duration::from_secs(5))?;
        verify_schema(&connection, initialize)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "recursive_triggers", true)?;
        let journal: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        let sync: u32 = connection.pragma_query_value(None, "synchronous", |r| r.get(0))?;
        let foreign_keys: bool =
            connection.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
        let recursive_triggers: bool =
            connection.pragma_query_value(None, "recursive_triggers", |r| r.get(0))?;
        if journal != "wal" || sync != 2 || !foreign_keys || !recursive_triggers {
            return Err(ArtifactStoreError::Catalog(
                "WAL/FULL durability or required SQLite constraints are unavailable",
            ));
        }
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if tx.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))? == 0 {
            tx.execute_batch(SCHEMA)?;
        }
        tx.commit()?;
        crate::runtime::platform_fs::sync_owned_file(&store.root.join(NAME))?;
        crate::runtime::platform_fs::complete_namespace_change(&store.root)?;
        Ok(Self(connection))
    }

    pub fn read(store: &ArtifactStore) -> Result<Snapshot, ArtifactStoreError> {
        let private = tempfile::Builder::new()
            .prefix("metewand-cache-snapshot-")
            .tempdir()?;
        crate::runtime::platform_fs::set_private_directory(private.path())?;
        let private_root = std::fs::canonicalize(private.path())?;
        crate::runtime::run_store::reader::copy_database_snapshot(&store.root, &private_root, NAME)?;
        let connection = Connection::open_with_flags(
            private_root.join(NAME),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.pragma_update(None, "query_only", true)?;
        verify_schema(&connection, false)?;
        Self(connection).snapshot()
    }

    pub fn snapshot(&self) -> Result<Snapshot, ArtifactStoreError> {
        let tx = self.0.unchecked_transaction()?;
        let integrity: String = tx.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        let foreign_keys: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |r| r.get(0),
        )?;
        if integrity != "ok" || foreign_keys {
            return Err(ArtifactStoreError::Catalog(
                "catalog integrity check failed",
            ));
        }
        let objects = tx
            .prepare("SELECT tree_sha256, manifest_sha256 FROM objects ORDER BY tree_sha256")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut roots: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in tx
            .prepare("SELECT tree_sha256, name FROM roots ORDER BY tree_sha256, name")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (digest, name) = row?;
            roots.entry(digest).or_default().push(name);
        }
        let pending = tx
            .prepare("SELECT tree_sha256, detached FROM gc_pending ORDER BY tree_sha256")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        tx.commit()?;
        Ok(Snapshot {
            objects,
            roots,
            pending,
        })
    }

    pub fn register(&self, digest: &str, marker: &str) -> Result<(), ArtifactStoreError> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO objects VALUES (?1, ?2) ON CONFLICT(tree_sha256) DO NOTHING",
            params![digest, marker],
        )?;
        let expected: String = tx.query_row(
            "SELECT manifest_sha256 FROM objects WHERE tree_sha256 = ?1",
            [digest],
            |r| r.get(0),
        )?;
        if expected != marker {
            return Err(ArtifactStoreError::ChangedContent);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn root(&self, digest: &str, name: &str, add: bool) -> Result<(), ArtifactStoreError> {
        let tx = self.0.unchecked_transaction()?;
        if add {
            tx.execute(
                "INSERT INTO roots VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                params![name, digest],
            )?;
        } else if tx.execute(
            "DELETE FROM roots WHERE name = ?1 AND tree_sha256 = ?2",
            params![name, digest],
        )? != 1
        {
            return Err(ArtifactStoreError::UnknownRoot(name.to_owned()));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn pending(&self, digest: &str, detached: bool) -> Result<(), ArtifactStoreError> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute("INSERT INTO gc_pending VALUES (?1, ?2) ON CONFLICT(tree_sha256) DO UPDATE SET detached = excluded.detached", params![digest, detached])?;
        tx.commit()?;
        Ok(())
    }

    pub fn forget(&self, digest: &str) -> Result<(), ArtifactStoreError> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute("DELETE FROM gc_pending WHERE tree_sha256 = ?1", [digest])?;
        tx.execute("DELETE FROM objects WHERE tree_sha256 = ?1", [digest])?;
        tx.commit()?;
        Ok(())
    }
}

type SchemaObject = (String, String, Option<String>);
fn schema_objects(connection: &Connection) -> rusqlite::Result<Vec<SchemaObject>> {
    connection.prepare("SELECT type, name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
}

fn verify_schema(connection: &Connection, initialize: bool) -> Result<(), ArtifactStoreError> {
    let version = connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?;
    let objects = schema_objects(connection)?;
    if initialize && version == 0 && objects.is_empty() {
        return Ok(());
    }
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA)?;
    if version != CACHE_CATALOG_VERSION || objects != schema_objects(&expected)? {
        return Err(ArtifactStoreError::Catalog(
            "unsupported cache catalog layout",
        ));
    }
    Ok(())
}
