//! Read-only projections of checked plans and committed run-store snapshots.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use metewand_core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, IdentityRecord},
    inspection::{
        AttemptStatus, Explanation, InspectionRecord, ObservationStatus, SlotState, SlotStatus,
        StatusReport, StatusSummary, diagnostic_id, referenced_ids,
    },
    planning::LogicalPlan,
    records::{AttemptSlotRole, IdentifiedRecord},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    local_tree::hash_local_tree,
    publication::{PublicationError, verify_finalized_directory},
    repository::CheckedRepository,
    run_store::{RunStoreError, reader::StoreSnapshot},
};

/// An in-memory inspection view. It never initializes, recovers, or writes a run.
#[derive(Default)]
pub struct Inspection {
    records: BTreeMap<String, InspectionRecord>,
    slots: Vec<SlotStatus>,
    attempts: Vec<AttemptStatus>,
    observations: Vec<ObservationStatus>,
    benchmark: Option<String>,
    manifest_hash: Option<String>,
    output_directory: Option<String>,
    current_slots: Option<BTreeSet<String>>,
}

impl Inspection {
    pub(crate) fn into_records(self) -> BTreeMap<String, InspectionRecord> {
        self.records
    }

    /// Adds the complete logical graph, including warm-ups and definitions.
    pub fn add_repository(
        &mut self,
        repository: &CheckedRepository,
        plan: &LogicalPlan,
    ) -> Result<(), InspectionError> {
        self.benchmark = Some(repository.manifest.name.to_string());
        self.manifest_hash = Some(repository.manifest_hash.to_string());
        self.records.extend(repository.resources.records.clone());
        let mut current_slots = BTreeSet::new();
        for experiment in &plan.experiments {
            self.add_identified(&experiment.execution_policy)?;
            self.add_identified(&experiment.observation_policy)?;
            for candidate in &experiment.candidates {
                self.add_identified(&candidate.dataset_configuration)?;
                self.add_identified(&candidate.problem_configuration)?;
                self.add_identified(&candidate.implementation_configuration)?;
                self.add_identified(&candidate.candidate)?;
                for specification in &candidate.specifications {
                    self.add_identified(&specification.specification)?;
                    for slot in &specification.observation_slots {
                        self.add_identified(slot)?;
                    }
                    for slot in &specification.attempt_slots {
                        self.add_identified(slot)?;
                        if matches!(slot.record.role, AttemptSlotRole::Measured { .. }) {
                            current_slots.insert(slot.id.to_string());
                            self.slots.push(SlotStatus {
                                logical_slot_id: slot.id.to_string(),
                                resolved_slot_id: None,
                                logical_run_id: specification.specification.id.to_string(),
                                resolved_run_id: None,
                                candidate_id: candidate.candidate.id.to_string(),
                                in_current_plan: Some(true),
                                state: SlotState::Pending,
                                attempt_ids: Vec::new(),
                                selected_observation_ids: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
        self.current_slots = Some(current_slots);
        Ok(())
    }

    fn add_identified<T: IdentityRecord>(
        &mut self,
        record: &IdentifiedRecord<T>,
    ) -> Result<(), InspectionError> {
        self.records
            .insert(record.id.to_string(), InspectionRecord::identified(record)?);
        Ok(())
    }

    /// Reads committed coordination and verifies every registered artifact.
    /// No acceptance is inferred from orphan directories or staging contents.
    pub fn add_store(&mut self, root: &Path) -> Result<(), InspectionError> {
        let snapshot = StoreSnapshot::read(root)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        self.output_directory = Some(snapshot.root.to_string_lossy().into_owned());
        for (&kind, rows) in &snapshot.tables {
            if matches!(kind, "event" | "attempt-artifact" | "observation-artifact") {
                continue;
            }
            for row in rows {
                let id = string(row, "id")?;
                let mut record = InspectionRecord::new(id.clone(), kind, row.clone());
                if kind == "stored-artifact" {
                    record.paths.push(string(row, "path")?);
                }
                self.records.insert(id, record);
            }
        }
        for artifact in &snapshot.tables["stored-artifact"] {
            self.read_artifact(&snapshot.root, artifact)?;
        }
        for row in &snapshot.tables["attempt"] {
            let id = string(row, "id")?;
            let finished_at_ns = optional_string(row, "finished_at_ns")?;
            let lease_expires_at_ns = optional_string(row, "lease_expires_at_ns")?;
            let recovery_required = finished_at_ns.is_none()
                && lease_expires_at_ns
                    .as_ref()
                    .map(|time| time.parse::<u128>())
                    .transpose()
                    .map_err(|_| InspectionError::Invalid("invalid lease timestamp"))?
                    .is_none_or(|expiry| expiry <= now);
            let mut diagnostic = None;
            if let Some(code) = row["failure_code"].as_str() {
                let record = &self.records[&id].record;
                let document = &record["document"];
                let value = if document["outcome"]["diagnostic"].is_object() {
                    document["outcome"]["diagnostic"].clone()
                } else {
                    json!({"code": code, "message": record["outcome"]["details"]["diagnostic"]["message"].as_str().unwrap_or("The attempt ended without an accepted outcome."), "details": record["outcome"]["details"]})
                };
                let diagnostic_identity = diagnostic_id(&value)?.to_string();
                let mut record =
                    InspectionRecord::new(diagnostic_identity.clone(), "diagnostic", value);
                record.relate([id.clone(), string(row, "slot_id")?]);
                if let Some(existing) = self.records.get_mut(&diagnostic_identity) {
                    existing.relate(record.related_ids);
                } else {
                    self.records.insert(diagnostic_identity.clone(), record);
                }
                diagnostic = Some(diagnostic_identity);
            }
            let events = snapshot.tables["event"]
                .iter()
                .filter(|event| event["attempt_id"] == id)
                .cloned()
                .collect::<Vec<_>>();
            let record = self.records.get_mut(&id).expect("registered attempt");
            record.record["events"] = json!(events);
            record.relate(diagnostic.iter().cloned());
            self.attempts.push(AttemptStatus {
                id,
                slot_id: string(row, "slot_id")?,
                retry_index: integer(row, "retry_index")?,
                state: string(row, "state")?,
                revision: integer(row, "revision")?,
                started_at_ns: string(row, "started_at_ns")?,
                updated_at_ns: string(row, "updated_at_ns")?,
                finished_at_ns,
                lease_expires_at_ns,
                recovery_required,
                artifact_id: optional_string(row, "artifact_id")?,
                diagnostic_id: diagnostic,
            });
        }
        for row in &snapshot.tables["observation"] {
            self.observations.push(ObservationStatus {
                id: string(row, "id")?,
                slot_id: string(row, "slot_id")?,
                attempt_id: string(row, "attempt_id")?,
                artifact_id: string(row, "artifact_id")?,
                valid: row["valid"]
                    .as_bool()
                    .ok_or(InspectionError::Invalid("invalid observation validity"))?,
                completion: string(row, "completion")?,
                selected: row["selected"]
                    .as_bool()
                    .ok_or(InspectionError::Invalid("invalid observation selection"))?,
            });
        }
        for kind in ["attempt-artifact", "observation-artifact"] {
            for row in &snapshot.tables[kind] {
                let ids = referenced_ids(row);
                for id in &ids {
                    if let Some(record) = self.records.get_mut(id) {
                        record.relate(ids.clone());
                    }
                }
            }
        }
        let stored_logical = snapshot.tables["resolved-attempt-slot"]
            .iter()
            .map(|row| string(row, "logical_slot_id"))
            .collect::<Result<BTreeSet<_>, _>>()?;
        self.slots
            .retain(|slot| !stored_logical.contains(&slot.logical_slot_id));
        for row in &snapshot.tables["resolved-attempt-slot"] {
            if row["role"] != "measurement" {
                continue;
            }
            let id = string(row, "id")?;
            let run_id = string(row, "resolved_run_id")?;
            let run = &self
                .records
                .get(&run_id)
                .ok_or(InspectionError::Invalid("slot has no resolved run"))?
                .record;
            let attempts = self
                .attempts
                .iter()
                .filter(|attempt| attempt.slot_id == id)
                .collect::<Vec<_>>();
            let state = if attempts.iter().any(|attempt| attempt.state == "accepted") {
                SlotState::Satisfied
            } else if attempts
                .iter()
                .any(|attempt| attempt.finished_at_ns.is_none() && !attempt.recovery_required)
            {
                SlotState::Active
            } else if attempts.iter().any(|attempt| attempt.recovery_required) {
                SlotState::RecoveryRequired
            } else if attempts.is_empty() {
                SlotState::Pending
            } else {
                SlotState::RetryRequired
            };
            let logical_slot_id = string(row, "logical_slot_id")?;
            self.slots.push(SlotStatus {
                in_current_plan: self
                    .current_slots
                    .as_ref()
                    .map(|slots| slots.contains(&logical_slot_id)),
                logical_slot_id,
                resolved_slot_id: Some(id),
                logical_run_id: string(run, "logical_specification_id")?,
                resolved_run_id: Some(run_id),
                candidate_id: string(run, "candidate_id")?,
                state,
                selected_observation_ids: snapshot.tables["observation"]
                    .iter()
                    .filter(|observation| {
                        observation["attempt_slot_id"] == row["id"]
                            && observation["selected"] == true
                    })
                    .map(|observation| string(observation, "id"))
                    .collect::<Result<_, _>>()?,
                attempt_ids: attempts.iter().map(|attempt| attempt.id.clone()).collect(),
            });
        }
        Ok(())
    }

    fn read_artifact(&mut self, root: &Path, artifact: &Value) -> Result<(), InspectionError> {
        let id = string(artifact, "id")?;
        let relative = string(artifact, "path")?;
        let path = confined(root, &relative)?;
        if artifact["storage_kind"] == "directory" {
            verify_finalized_directory(&path)?;
            if hex(hash_local_tree(&path)
                .map_err(PublicationError::from)?
                .bytes())
                != artifact["sha256"]
                || hex(&Sha256::digest(fs::read(path.join("manifest.json"))?))
                    != artifact["manifest_sha256"]
            {
                return Err(InspectionError::CorruptArtifact(id));
            }
            self.records
                .get_mut(&id)
                .expect("registered artifact")
                .record["manifest"] = optional_document(&path.join("manifest.json"))?
                .ok_or(InspectionError::Invalid("missing artifact inventory"))?;
        } else if artifact["storage_kind"] == "file" {
            if hex(&Sha256::digest(fs::read(&path)?)) != artifact["sha256"] {
                return Err(InspectionError::CorruptArtifact(id));
            }
            return Ok(());
        } else {
            return Err(InspectionError::Invalid("unknown artifact storage kind"));
        }
        for (file, kind) in [
            ("attempt.json", "attempt"),
            ("observation.json", "observation"),
        ] {
            if artifact["kind"] != kind {
                continue;
            }
            if let Some(document) = optional_document(&path.join(file))? {
                let subject_id = string(&document, "id")?;
                // Artifact registration can precede semantic observation
                // registration. Those bytes do not establish an observation.
                let Some(record) = self.records.get_mut(&subject_id) else {
                    continue;
                };
                record.record["document"] = document.clone();
                record.paths.push(format!("{relative}/{file}"));
                record.relate([id.clone()]);
                self.read_provenance(&document)?;
                if kind == "observation" {
                    for role in ["result", "metrics"] {
                        let reference = &document[role];
                        let content_id = string(reference, "id")?;
                        let mut content = InspectionRecord::new(
                            content_id.clone(),
                            "artifact",
                            json!({"role": role, "reference": reference, "stored_artifact_id": id}),
                        );
                        let manifest_path = if role == "metrics" {
                            string(reference, "path")?
                        } else {
                            string(&document["provenance"]["data"], "result_manifest")?
                        };
                        let manifest = confined(&path, &manifest_path)?;
                        content.record["document"] = optional_document(&manifest)?.ok_or(
                            InspectionError::Invalid("missing result or metrics document"),
                        )?;
                        content
                            .paths
                            .push(format!("{relative}/{}", string(reference, "path")?));
                        content.relate([id.clone(), subject_id.clone()]);
                        if let Some(existing) = self.records.get_mut(&content_id) {
                            existing.paths.extend(content.paths);
                            existing.relate(content.related_ids);
                        } else {
                            self.records.insert(content_id, content);
                        }
                    }
                }
            }
        }
        if artifact["kind"] == "attempt" {
            let outcome = optional_document(&path.join("outcome.json"))?.ok_or(
                InspectionError::Invalid("attempt artifact lacks its outcome record"),
            )?;
            let attempt_id = string(&outcome, "attempt_id")?;
            let record = self
                .records
                .get_mut(&attempt_id)
                .ok_or(InspectionError::Invalid("outcome has no committed attempt"))?;
            if record.record["state"] != outcome["state"]
                || record.record["slot_id"] != outcome["slot_id"]
                || record.record["artifact_id"] != id
            {
                return Err(InspectionError::CorruptArtifact(id));
            }
            record.record["outcome"] = outcome;
            record.paths.push(format!("{relative}/outcome.json"));
            record.relate([id]);
        }
        Ok(())
    }

    fn read_provenance(&mut self, document: &Value) -> Result<(), InspectionError> {
        let context = &document["provenance"]["data"]["context"];
        for (key, kind) in [
            ("dataset_instance", "dataset-instance"),
            ("problem_instance", "problem-instance"),
            ("resolved_specification", "resolved-specification"),
            ("resolved_attempt_slot", "resolved-attempt-slot"),
            ("resolved_observation_slot", "resolved-observation-slot"),
            ("execution_policy", "execution-policy"),
        ] {
            self.add_representation(kind, &context[key])?;
        }
        for role in ["implementation", "evaluator"] {
            for (key, kind) in [
                ("environment", "resolved-environment"),
                ("launch", "resolved-launch"),
                ("worker", "resolved-worker"),
            ] {
                self.add_representation(kind, &context[role][key])?;
            }
        }
        Ok(())
    }

    fn add_representation(
        &mut self,
        kind: &str,
        representation: &Value,
    ) -> Result<(), InspectionError> {
        if representation.is_null() {
            return Ok(());
        }
        if representation["version"] != metewand_core::IDENTITY_VERSION {
            return Err(InspectionError::Invalid(
                "unsupported provenance identity version",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(kind.as_bytes());
        digest.update([0]);
        digest.update(CanonicalValue::try_from(representation.clone())?.to_canonical_bytes()?);
        let id = format!("mw1-{kind}-{}", hex(&digest.finalize()));
        self.records
            .entry(id.clone())
            .or_insert_with(|| InspectionRecord::new(id, kind, representation.clone()));
        Ok(())
    }

    fn connect_records(&mut self) {
        let edges = self
            .records
            .iter()
            .map(|(id, record)| (id.clone(), referenced_ids(&record.record)))
            .collect::<Vec<_>>();
        for (id, related) in edges {
            for other in related {
                if other == id {
                    continue;
                }
                let kind = other
                    .strip_prefix("mw1-")
                    .and_then(|value| value.rsplit_once('-'))
                    .map(|(kind, _)| kind)
                    .unwrap_or("store-reference")
                    .to_owned();
                self.records
                    .entry(other.clone())
                    .or_insert_with(|| {
                        InspectionRecord::new(
                            other.clone(),
                            kind,
                            json!({"availability": "reference_only"}),
                        )
                    })
                    .relate([id.clone()]);
                self.records
                    .get_mut(&id)
                    .expect("existing record")
                    .relate([other]);
            }
        }
        for record in self.records.values_mut() {
            record.paths.sort();
            record.paths.dedup();
        }
    }

    /// Exact lookup never accepts a prefix or falls back to a manifest name.
    pub fn explain(mut self, id: &str) -> Result<Explanation, InspectionError> {
        self.connect_records();
        let subject = self
            .records
            .remove(id)
            .ok_or_else(|| InspectionError::UnknownIdentity(id.to_owned()))?;
        Ok(Explanation {
            command: "explain",
            subject,
        })
    }

    pub fn status(mut self) -> StatusReport {
        self.connect_records();
        self.slots.sort_by(|left, right| {
            (&left.logical_slot_id, &left.resolved_slot_id)
                .cmp(&(&right.logical_slot_id, &right.resolved_slot_id))
        });
        let mut summary = StatusSummary {
            attempt_slots: self.slots.len(),
            ..StatusSummary::default()
        };
        for slot in &self.slots {
            match slot.state {
                SlotState::Pending => summary.pending_slots += 1,
                SlotState::Active => summary.active_slots += 1,
                SlotState::Satisfied => summary.satisfied_slots += 1,
                SlotState::RetryRequired => summary.retry_required_slots += 1,
                SlotState::RecoveryRequired => summary.recovery_required_slots += 1,
            }
        }
        for attempt in &self.attempts {
            match attempt.state.as_str() {
                "accepted" => summary.accepted_attempts += 1,
                "failed" => summary.failed_attempts += 1,
                "superseded" => summary.superseded_attempts += 1,
                "censored" => summary.censored_attempts += 1,
                _ => summary.active_attempts += 1,
            }
        }
        summary.selected_observations = self
            .observations
            .iter()
            .filter(|observation| observation.selected)
            .count();
        StatusReport {
            command: "status",
            benchmark: self.benchmark,
            manifest_hash: self.manifest_hash,
            output_directory: self.output_directory,
            slots: self.slots,
            attempts: self.attempts,
            observations: self.observations,
            diagnostics: self
                .records
                .into_values()
                .filter(|record| record.kind == "diagnostic")
                .collect(),
            summary,
        }
    }
}

fn string(value: &Value, key: &str) -> Result<String, InspectionError> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or(InspectionError::Invalid("missing string in stored record"))
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>, InspectionError> {
    if value[key].is_null() {
        Ok(None)
    } else {
        string(value, key).map(Some)
    }
}

fn integer(value: &Value, key: &str) -> Result<u64, InspectionError> {
    value[key]
        .as_u64()
        .ok_or(InspectionError::Invalid("invalid integer in stored record"))
}

fn optional_document(path: &Path) -> Result<Option<Value>, InspectionError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(CanonicalValue::from_slice(&bytes)?.as_json().clone())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn confined(root: &Path, relative: &str) -> Result<PathBuf, InspectionError> {
    if relative.is_empty() {
        return Err(InspectionError::Invalid("empty artifact path"));
    }
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(InspectionError::Invalid("unsafe artifact path"));
        };
        path.push(component);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(InspectionError::Invalid(
                "artifact path traverses a symbolic link",
            ));
        }
    }
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Error)]
pub enum InspectionError {
    #[error("identity `{0}` was not found in the selected plan or run store")]
    UnknownIdentity(String),
    #[error("invalid inspection evidence: {0}")]
    Invalid(&'static str),
    #[error("committed artifact `{0}` does not match its recorded contents")]
    CorruptArtifact(String),
    #[error(transparent)]
    Store(#[from] RunStoreError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error(transparent)]
    Publication(#[from] PublicationError),
}
