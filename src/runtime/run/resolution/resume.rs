//! Verification of committed input identities when no selected slot needs execution.

use super::*;
use crate::core::{
    manifest::RemoteSource,
    materialization::{ProblemPreparationRecord, VerifiedSourceRecord},
    records::{DatasetConfigurationRecord, ProblemConfigurationRecord},
};

#[derive(Default)]
pub(super) struct CommittedInputs {
    datasets: BTreeMap<RecordId<DatasetConfigurationRecord>, RecordId<DatasetArtifact>>,
    preparations: BTreeMap<
        (
            RecordId<DatasetInstanceRecord>,
            RecordId<ProblemConfigurationRecord>,
        ),
        RecordId<DatasetArtifact>,
    >,
}

impl CommittedInputs {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn accepted(
        &mut self,
        repository: &CheckedRepository,
        store: &RunStore,
        candidate: &LogicalCandidatePlan,
        implementation: &ResolvedWorker,
        evaluator: &ResolvedWorker,
        dataset: &DatasetPlan<'_>,
        preparation: Option<&ResolvedWorker>,
        executor: RecordId<ExecutorDefinitionRecord>,
    ) -> Result<bool, RunError> {
        let measured = candidate.specifications.iter().flat_map(|specification| {
            specification
                .attempt_slots
                .iter()
                .filter(|slot| matches!(slot.record.role, AttemptSlotRole::Measured { .. }))
                .map(move |slot| (specification.specification.id, slot.id))
        });
        let Some((_, first_slot)) = measured.clone().next() else {
            return Ok(false);
        };

        for context in store.accepted_input_contexts(&first_slot.to_string())? {
            let Some(dataset_instance) =
                dataset_instance(repository, candidate, dataset, &context)?
            else {
                continue;
            };
            let artifact = dataset_instance.record.artifact;
            let configuration = candidate.dataset_configuration.id;
            if self
                .datasets
                .get(&configuration)
                .is_some_and(|known| *known != artifact)
            {
                continue;
            }
            let problem = candidate.problem_configuration.id;
            let preparation_key = (dataset_instance.id, problem);
            let prepared = if let Some(worker) = preparation {
                let derivation = identify_record(ProblemPreparationRecord {
                    dataset: dataset_instance.id,
                    problem,
                    preparer: worker.record.clone(),
                })?;
                let Some(artifact) = receipt_artifact(
                    &context["problem_preparation"],
                    &record_document(&derivation.record)?,
                    &derivation.id.to_string(),
                )?
                else {
                    continue;
                };
                if self
                    .preparations
                    .get(&preparation_key)
                    .is_some_and(|known| *known != artifact)
                {
                    continue;
                }
                Some(artifact)
            } else {
                None
            };
            let problem_instance = identify_record(ProblemInstanceRecord {
                dataset: dataset_instance.id,
                configuration: problem,
                preparation: prepared,
            })?;
            if context["problem_instance"] != record_document(&problem_instance.record)? {
                continue;
            }
            let mut all_accepted = true;
            for (logical_specification, logical_slot) in measured.clone() {
                let specification = resolved_specification(
                    logical_specification,
                    problem_instance.id,
                    implementation.record.clone(),
                    evaluator.record.clone(),
                    executor,
                    None,
                )?;
                let slot = identify_record(ResolvedAttemptSlotRecord {
                    logical_slot,
                    specification: specification.id,
                })?;
                if !store.slot_is_accepted(slot.id)? {
                    all_accepted = false;
                    break;
                }
            }
            if all_accepted {
                match dataset {
                    DatasetPlan::Ready(snapshot, _) => snapshot.verify()?,
                    DatasetPlan::Generated(_, worker) => worker.verify()?,
                }
                if let Some(worker) = preparation {
                    worker.verify()?;
                }
                implementation.verify()?;
                evaluator.verify()?;
                self.datasets.insert(configuration, artifact);
                if let Some(artifact) = prepared {
                    self.preparations.insert(preparation_key, artifact);
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn dataset_instance(
    repository: &CheckedRepository,
    candidate: &LogicalCandidatePlan,
    dataset: &DatasetPlan<'_>,
    context: &Value,
) -> Result<Option<IdentifiedRecord<DatasetInstanceRecord>>, RunError> {
    let (artifact, output_schema) = match dataset {
        DatasetPlan::Ready(snapshot, schema) => (snapshot.artifact()?, *schema),
        DatasetPlan::Generated(definition, materializer) => {
            let receipt = &context["dataset_materialization"];
            let derivation = identify_dataset_derivation(
                &candidate.dataset_configuration.record,
                None,
                &materializer.record,
            )?;
            let mut derivation = derivation.record;
            if let Some(source) = &definition.source {
                let Some(verified) = pinned_source(receipt, source) else {
                    return Ok(None);
                };
                derivation.sources.push(verified);
            }
            let derivation = identify_record(derivation)?;
            let Some(artifact) = receipt_artifact(
                receipt,
                &record_document(&derivation.record)?,
                &derivation.id.to_string(),
            )?
            else {
                return Ok(None);
            };
            (
                artifact,
                repository.resources.schemas[definition.output_schema.as_path()],
            )
        }
    };
    let record = DatasetInstanceRecord {
        configuration: candidate.dataset_configuration.id,
        artifact,
        output_schema,
    };
    if context["dataset_instance"] != record_document(&record)? {
        return Ok(None);
    }
    Ok(Some(identify_record(record)?))
}

fn pinned_source(receipt: &Value, source: &RemoteSource) -> Option<VerifiedSourceRecord> {
    let files = receipt["source"]["files"].as_array()?;
    let [file] = files.as_slice() else {
        return None;
    };
    let sources = receipt["derivation"]["sources"].as_array()?;
    let [verified] = sources.as_slice() else {
        return None;
    };
    if receipt["source"]["raw_bytes_verified"] != true
        || file["path"] != "source"
        || file["sha256"] != source.sha256.as_str()
    {
        return None;
    }
    let digest = sha256_text(&verified["tree_sha256"])?;
    let mut bytes = [0; 32];
    for (byte, offset) in bytes.iter_mut().zip((0..digest.len()).step_by(2)) {
        *byte = u8::from_str_radix(&digest[offset..offset + 2], 16).ok()?;
    }
    Some(VerifiedSourceRecord {
        selection: canonical(json!({
            "kind": "pinned_url", "url": source.url, "sha256": source.sha256.as_str(),
        }))
        .ok()?,
        tree_hash_version: LOCAL_TREE_HASH_VERSION,
        tree_digest: ContentDigest::new(bytes),
        decoding: None,
    })
}

fn receipt_artifact(
    receipt: &Value,
    derivation: &Value,
    derivation_id: &str,
) -> Result<Option<RecordId<DatasetArtifact>>, RunError> {
    if receipt["version"] != 1
        || receipt["derivation"] != *derivation
        || receipt["derivation_id"] != derivation_id
        || receipt["output"]["tree_hash_version"] != LOCAL_TREE_HASH_VERSION
    {
        return Ok(None);
    }
    let Some(digest) = sha256_text(&receipt["output"]["sha256"]) else {
        return Ok(None);
    };
    Ok(Some(identify_canonical(&canonical(json!({
        "tree_hash_version": LOCAL_TREE_HASH_VERSION, "tree_sha256": digest,
    }))?)?))
}

fn sha256_text(value: &Value) -> Option<&str> {
    value.as_str().filter(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}
