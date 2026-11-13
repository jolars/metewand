//! Shared immutable transaction journal with disposable host-local SQLite views.

use rusqlite::{Connection, Transaction, TransactionBehavior, params_from_iter};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::{RunStore, RunStoreError, attempts::invalid};
use crate::core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    durable::{DURABLE_RECORD_VERSION, DurableChange, DurableRecord, DurableRow},
};

const JOURNAL: &str = "journal";

#[derive(Debug)]
pub(super) struct SharedJournal {
    directory: PathBuf,
    next: u64,
    previous: Option<String>,
    failed: bool,
    // Reconnected attempt and heartbeat stores must retain both resources.
    _writer: File,
    _scratch: tempfile::TempDir,
}

pub(crate) fn is_shared(root: &Path) -> bool {
    fs::symlink_metadata(root.join(JOURNAL)).is_ok()
}

impl RunStore {
    /// Opens a shared run with exactly one coordinator and a fresh local view.
    /// Shared storage must support locking, file synchronization, directory
    /// synchronization on Unix, and atomic no-replace rename. Filesystem-specific
    /// multi-host and server power-loss qualification is a separate requirement.
    pub fn open_shared(root: &Path) -> Result<Self, RunStoreError> {
        if !root.is_absolute()
            || !crate::runtime::platform_fs::is_real_directory(&fs::symlink_metadata(root)?)
        {
            return Err(RunStoreError::UnsafePath(root.to_owned()));
        }
        if root.join("run.sqlite3").exists() || crate::runtime::archive::is_collection(root) {
            return Err(invalid(
                "shared storage requires a new run, not an existing SQLite run or archive collection",
            ));
        }
        let root = fs::canonicalize(root)?;
        let writer = crate::runtime::platform_fs::open_regular(
            &root.join(".shared-writer.lock"),
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false),
        )?;
        match writer.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => return Err(RunStoreError::Busy),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        crate::runtime::durability::sync_all(&writer)?;
        let directory = root.join(JOURNAL);
        match fs::create_dir(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        real_directory(&directory)?;
        crate::runtime::platform_fs::complete_namespace_change(&root)?;
        // A crashed publisher may have renamed a complete record without
        // synchronizing its parent. Settle that namespace before reusing it.
        crate::runtime::platform_fs::complete_namespace_change(&directory)?;
        let scratch = tempfile::Builder::new()
            .prefix("metewand-coordination-")
            .tempdir()?;
        let mut store = Self::open_fresh(&fs::canonicalize(scratch.path())?)?;
        if store.root().starts_with(&root) {
            return Err(invalid(
                "shared runs require TMPDIR outside the durable run home",
            ));
        }
        let (next, previous) = replay(&directory, &mut store.connection)?;
        let mut journal = SharedJournal {
            directory,
            next,
            previous,
            failed: false,
            _writer: writer,
            _scratch: scratch,
        };
        if journal.next == 0 {
            journal.publish(Vec::new())?;
        }
        install_capture(&store.connection)?;
        store.root = root;
        store.shared = Some(Arc::new(Mutex::new(journal)));
        Ok(store)
    }

    pub(super) fn transaction(&mut self) -> Result<StoreTransaction<'_>, RunStoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(StoreTransaction {
            tx,
            shared: self.shared.as_ref(),
        })
    }
}

pub(super) struct StoreTransaction<'a> {
    tx: Transaction<'a>,
    shared: Option<&'a Arc<Mutex<SharedJournal>>>,
}

impl<'a> Deref for StoreTransaction<'a> {
    type Target = Transaction<'a>;
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

impl StoreTransaction<'_> {
    pub fn commit(self) -> Result<(), RunStoreError> {
        let Some(shared) = self.shared else {
            return Ok(self.tx.commit()?);
        };
        let mut journal = shared
            .lock()
            .map_err(|_| invalid("shared journal lock was poisoned"))?;
        if journal.failed {
            return Err(invalid(
                "shared publication failed; reopen the run to reconstruct local state",
            ));
        }
        let texts = self.tx.prepare("SELECT collection,before_json,after_json FROM temp.durable_changes ORDER BY sequence")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        let changes = texts
            .into_iter()
            .map(|(collection, before, after)| {
                let parse = |s: Option<String>| -> Result<Option<DurableRow>, RunStoreError> {
                    s.map(|s| {
                        serde_json::from_str(&s)
                            .map_err(|_| invalid("invalid captured coordination record"))
                    })
                    .transpose()
                };
                Ok(DurableChange {
                    collection,
                    before: parse(before)?,
                    after: parse(after)?,
                })
            })
            .collect::<Result<Vec<_>, RunStoreError>>()?;
        // Check deferred constraints before publication makes this transaction
        // authoritative. The local commit may then fail without revoking it.
        if self.tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(invalid("coordination transaction violates foreign keys"));
        }
        self.tx.execute("DELETE FROM temp.durable_changes", [])?;
        if !changes.is_empty()
            && let Err(error) = journal.publish(changes)
        {
            journal.failed = true;
            return Err(error);
        }
        #[cfg(test)]
        crate::runtime::fault_injection::hit("shared.local_commit.before");
        if let Err(error) = self.tx.commit() {
            journal.failed = true;
            return Err(error.into());
        }
        #[cfg(test)]
        crate::runtime::fault_injection::hit("shared.local_commit.after");
        Ok(())
    }
}

impl SharedJournal {
    fn publish(&mut self, changes: Vec<DurableChange>) -> Result<(), RunStoreError> {
        if self.next > MAX_SAFE_INTEGER {
            return Err(invalid("shared journal sequence exhausted"));
        }
        let record = DurableRecord {
            schema_version: DURABLE_RECORD_VERSION,
            sequence: self.next,
            previous_sha256: self.previous.clone(),
            changes,
        };
        let bytes =
            CanonicalValue::try_from(serde_json::to_value(record).expect("durable record"))?
                .to_canonical_bytes()?;
        let mut stage = tempfile::Builder::new()
            .prefix(".pending-")
            .tempfile_in(&self.directory)?;
        stage.write_all(&bytes)?;
        crate::runtime::durability::sync_all(stage.as_file())?;
        #[cfg(test)]
        crate::runtime::fault_injection::hit("shared.publish.before");
        crate::runtime::publication::rename_no_replace(
            stage.path(),
            &self.directory.join(filename(self.next, &digest(&bytes))),
        )?;
        crate::runtime::platform_fs::complete_namespace_change(&self.directory)?;
        #[cfg(test)]
        crate::runtime::fault_injection::hit("shared.publish.after");
        self.previous = Some(digest(&bytes));
        self.next += 1;
        Ok(())
    }
}

pub(super) fn install_capture(db: &Connection) -> Result<(), RunStoreError> {
    db.execute_batch("CREATE TEMP TABLE durable_changes (sequence INTEGER PRIMARY KEY, collection TEXT NOT NULL, before_json TEXT, after_json TEXT);")?;
    for (name, columns) in collections(db)? {
        let row = |prefix: &str| {
            let fields = columns
                .iter()
                .map(|c| format!("'{c}', CAST({prefix}.{c} AS TEXT)"))
                .collect::<Vec<_>>()
                .join(",");
            format!("json_object({fields})")
        };
        for (operation, before, after) in [
            ("INSERT", "NULL".to_owned(), row("NEW")),
            ("UPDATE", row("OLD"), row("NEW")),
            ("DELETE", row("OLD"), "NULL".to_owned()),
        ] {
            db.execute_batch(&format!("CREATE TEMP TRIGGER durable_{name}_{operation} AFTER {operation} ON main.{name} BEGIN INSERT INTO durable_changes(collection,before_json,after_json) VALUES ('{name}',{before},{after}); END;"))?;
        }
    }
    Ok(())
}

// Only tables in the exact supported coordination layout can be captured or
// replayed. Journal bytes never supply executable SQL or schema definitions.
fn collections(db: &Connection) -> Result<BTreeMap<String, Vec<String>>, RunStoreError> {
    let names = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    names
        .into_iter()
        .map(|name| {
            let columns = db
                .prepare(&format!("PRAGMA table_info({name})"))?
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((name, columns))
        })
        .collect()
}

/// Builds a private view for read-only inspection without touching shared files.
pub(super) fn read_shared(root: &Path, db: &mut Connection) -> Result<(), RunStoreError> {
    db.execute_batch(super::FRESH_RUN_STORE_SCHEMA_SQL)?;
    db.pragma_update(None, "foreign_keys", true)?;
    let (next, _) = replay(&root.join(JOURNAL), db)?;
    if next == 0 {
        return Err(invalid("shared run has no committed initialization record"));
    }
    Ok(())
}

fn replay(directory: &Path, db: &mut Connection) -> Result<(u64, Option<String>), RunStoreError> {
    real_directory(directory)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_some_and(|s| s.starts_with(".pending-")) {
            continue;
        }
        paths.push(entry.path());
    }
    paths.sort();
    let columns = collections(db)?;
    // Lifecycle triggers govern new execution. Replay checks before-images,
    // strict row constraints, and foreign keys at every original commit boundary.
    let triggers = db
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger' ORDER BY name")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (name, _) in &triggers {
        db.execute_batch(&format!("DROP TRIGGER {name}"))?;
    }
    let mut previous = None;
    let mut next = 0;
    for path in paths {
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(&format!("{next:020}-")))
        {
            return Err(invalid(
                "shared journal has a gap or unexpected committed entry",
            ));
        }
        let mut file =
            crate::runtime::platform_fs::open_regular(&path, OpenOptions::new().read(true))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)?;
        if path.file_name() != Some(std::ffi::OsStr::new(&filename(next, &digest(&bytes)))) {
            return Err(invalid(
                "shared record content hash differs from its filename",
            ));
        }
        let canonical = CanonicalValue::from_slice(&bytes)?;
        if canonical.to_canonical_bytes()? != bytes {
            return Err(invalid("shared record is not canonical JSON"));
        }
        let record: DurableRecord = serde_json::from_value(canonical.as_json().clone())
            .map_err(|_| invalid("invalid shared transaction record"))?;
        if record.schema_version != DURABLE_RECORD_VERSION {
            return Err(invalid("unsupported shared record version"));
        }
        if record.sequence != next
            || record.previous_sha256 != previous
            || (next == 0 && !record.changes.is_empty())
        {
            return Err(invalid(
                "shared journal sequence or predecessor hash differs",
            ));
        }
        let tx = db.transaction()?;
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for change in &record.changes {
            apply(&tx, &columns, change)?;
        }
        tx.commit()?;
        previous = Some(digest(&bytes));
        next += 1;
    }
    for (_, sql) in triggers {
        db.execute_batch(&sql)?;
    }
    super::durable::verify_schema(db)?;
    Ok((next, previous))
}

fn apply(
    tx: &Transaction<'_>,
    collections: &BTreeMap<String, Vec<String>>,
    change: &DurableChange,
) -> Result<(), RunStoreError> {
    let columns = collections
        .get(&change.collection)
        .ok_or_else(|| invalid("unknown shared record collection"))?;
    let validate = |row: &DurableRow| {
        row.len() == columns.len() && columns.iter().all(|c| row.contains_key(c))
    };
    if change.before.as_ref().is_some_and(|r| !validate(r))
        || change.after.as_ref().is_some_and(|r| !validate(r))
        || (change.before.is_none() && change.after.is_none())
    {
        return Err(invalid(
            "shared row fields differ from the versioned contract",
        ));
    }
    let table = &change.collection;
    if let Some(before) = &change.before {
        let predicate = columns
            .iter()
            .enumerate()
            .map(|(i, c)| format!("CAST({c} AS TEXT) IS ?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let count = tx.execute(
            &format!("DELETE FROM {table} WHERE {predicate}"),
            params_from_iter(columns.iter().map(|c| &before[c])),
        )?;
        if count != 1 {
            return Err(invalid(
                "shared record before-image differs from committed state",
            ));
        }
    }
    if let Some(after) = &change.after {
        let placeholders = (1..=columns.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        tx.execute(
            &format!(
                "INSERT INTO {table} ({}) VALUES ({placeholders})",
                columns.join(",")
            ),
            params_from_iter(columns.iter().map(|c| &after[c])),
        )?;
        let predicate = columns
            .iter()
            .enumerate()
            .map(|(i, c)| format!("CAST({c} AS TEXT) IS ?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let exact: i64 = tx.query_row(
            &format!("SELECT count(*) FROM {table} WHERE {predicate}"),
            params_from_iter(columns.iter().map(|c| &after[c])),
            |r| r.get(0),
        )?;
        if exact != 1 {
            return Err(invalid(
                "shared record after-image changes under storage normalization",
            ));
        }
    }
    Ok(())
}

fn real_directory(path: &Path) -> Result<(), RunStoreError> {
    if !crate::runtime::platform_fs::is_real_directory(&fs::symlink_metadata(path)?) {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(())
}

fn filename(sequence: u64, digest: &str) -> String {
    format!("{sequence:020}-{digest}.json")
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
