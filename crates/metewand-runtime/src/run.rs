//! Manifest-driven execution of the trusted local Gate-1 vertical path.

mod attempt;
mod buffered;
mod check;

pub use check::{
    ReferenceCaseCheck, WorkerCheck, WorkerCheckError, WorkerCheckReport, check_repository_workers,
};
mod resolution;
mod selection;

use crate::run_store::{AttemptPhase, RunStore};
use attempt::AttemptExecution;

pub use metewand_core::run_output::RunOptions;

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
    time::SystemTime,
};

use metewand_core::{
    EXECUTION_SEMANTICS_VERSION,
    canonical::CanonicalJsonError,
    identity::IdentityError,
    measurements::ExecutionMeasurements,
    planning::LogicalPlan,
    public_schemas::PublicSchemaCatalogError,
    records::{AttemptDiagnostic, ProvenanceRecord, RunAttemptOutcome, RunAttemptRecord},
    run_output::{
        RunAttemptOutput, RunAttemptStatus, RunDiagnostic, RunEvent, RunEventKind, RunReport,
        RunSummary,
    },
};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    fixed_dataset_run::{
        ExecutionTimingPolicy, FixedDatasetRun, FixedDatasetRunError, run_fixed_dataset_in_view,
        run_fixed_dataset_with_session,
    },
    local_tree::LocalTreeHashError,
    publication::{PublicationError, PublicationRequest},
    repository::CheckedRepository,
    source_bundle::{SourceBundleError, SourceBundleIdentityError},
    source_resolution::SourceResolutionError,
    worker_output::WorkerOutputError,
    worker_process::WorkerLogLimits,
};

use resolution::{ResolvedCandidate, ResolvedMeasurement, canonical};

/// Executes the deterministic schedule with policy-bounded concurrency.
///
/// `output_directory` must name a new directory beneath an existing parent.
/// Preflight validates the complete selection before launching any worker or
/// creating that directory. Inputs are copied to private temporary storage;
/// resolved identities bind their contents and normalized launch templates,
/// while actual host paths remain provenance. Every attempt is published before
/// its terminal event. A failed attempt does not stop later selected slots.
///
/// Starts and terminal events follow deterministic dispatch batches. The sink
/// runs on the calling thread before dispatch and after all attempts in a batch
/// have finished. Sink failure stops further dispatch; already dispatched
/// attempts finish and retain their durably published outcomes.
///
/// # Errors
///
/// Returns a preflight, input, publication, or event-delivery error. Worker and
/// scientific failures instead become typed failed attempts in the report.
pub fn run_repository(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    output_directory: &Path,
    event_sink: impl FnMut(&RunEvent) -> io::Result<()>,
) -> Result<RunReport, RunError> {
    run_repository_with_cache(repository, plan, output_directory, None, event_sink)
}

/// Executes with an optional explicitly owned persistent artifact cache.
/// Without a cache directory, the run uses an owned temporary cache. Cache
/// creation follows complete selection preflight. Existing completed dataset
/// constructions are verified and reused without launching the materializer.
pub fn run_repository_with_cache(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    output_directory: &Path,
    cache_directory: Option<&Path>,
    event_sink: impl FnMut(&RunEvent) -> io::Result<()>,
) -> Result<RunReport, RunError> {
    run_repository_with_options(
        repository,
        plan,
        output_directory,
        cache_directory,
        &RunOptions::default(),
        event_sink,
    )
}

/// Executes a selected, bounded invocation, optionally resuming durable history.
/// Accepted slots are never reserved again. Explicit retries get new identities.
pub fn run_repository_with_options(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    output_directory: &Path,
    cache_directory: Option<&Path>,
    options: &RunOptions,
    mut event_sink: impl FnMut(&RunEvent) -> io::Result<()>,
) -> Result<RunReport, RunError> {
    let plan = selection::preselect(plan, options)?;
    let output_directory = destination(output_directory, options.resume)?;
    let cache_directory = cache_directory
        .map(|path| {
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                std::env::current_dir()?.join(path)
            };
            if fs::symlink_metadata(&path).is_ok() {
                if !fs::symlink_metadata(&path)?.is_dir() {
                    return Err(io::Error::other("cache root must be a real directory"));
                }
                fs::canonicalize(&path)
            } else {
                Ok(fs::canonicalize(
                    path.parent()
                        .ok_or_else(|| io::Error::other("cache root needs an existing parent"))?,
                )?
                .join(
                    path.file_name()
                        .ok_or_else(|| io::Error::other("cache root needs a name"))?,
                ))
            }
        })
        .transpose()?;
    let mut resolved = resolution::resolve(
        repository,
        &plan,
        &output_directory,
        cache_directory.as_deref(),
    )?;
    for candidate in &resolved.candidates {
        candidate.validate_destination(&output_directory)?;
    }
    let mut store = if options.resume {
        Some(RunStore::open(&output_directory)?)
    } else {
        None
    };
    let mut selected = std::collections::BTreeSet::new();
    if options.selected_ids.is_empty() {
        selected.extend(resolved.schedule.iter().copied());
    }
    for id in &options.selected_ids {
        let previous = store
            .as_ref()
            .map(|store| store.slots_for_record(id))
            .transpose()?
            .unwrap_or_default();
        let mut matched = resolved.candidates.iter().any(|candidate| {
            candidate.applicability.record.outcome
                == metewand_core::applicability::ApplicabilityOutcome::NotApplicable
                && (candidate.logical_selection_ids.contains(id)
                    || candidate.applicability.id.to_string() == *id)
        });
        for &(candidate, measurement) in &resolved.schedule {
            let slot = &resolved.candidates[candidate].measurements[measurement];
            if slot.selection_ids.contains(id) || previous.contains(&slot.attempt_slot.id) {
                selected.insert((candidate, measurement));
                matched = true;
            }
        }
        if !matched {
            return Err(RunError::Selection(format!(
                "selected identity `{id}` matches no measured slot"
            )));
        }
    }
    if !options.resume {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&output_directory)?;
        fs::set_permissions(&output_directory, fs::Permissions::from_mode(0o700))?;
        fs::File::open(output_directory.parent().expect("destination has a parent"))?.sync_all()?;
        store = Some(
            if resolved
                .candidates
                .iter()
                .any(|c| c.fresh_profile.is_some())
            {
                RunStore::open_fresh(&output_directory)?
            } else if resolved.candidates.iter().any(|c| c.profile.is_some()) {
                RunStore::open_buffered(&output_directory)?
            } else {
                RunStore::open(&output_directory)?
            },
        );
    }
    let mut store = store.expect("new or resumed store");
    if options.resume {
        store.recover(attempt::now)?;
    }
    for candidate in &resolved.candidates {
        if candidate.applicability.record.checked_inputs.is_some() {
            store.register_applicability(&candidate.applicability)?;
        }
    }
    let fresh_repetitions = selected
        .iter()
        .filter_map(|&(candidate, measurement)| {
            let slot = &resolved.candidates[candidate].measurements[measurement];
            slot.fresh.as_ref().map(|_| {
                (
                    candidate,
                    slot.specification.id,
                    slot.scheduling.comparison_block.measurement_index,
                )
            })
        })
        .collect::<std::collections::BTreeSet<_>>();
    for (index, candidate) in resolved.candidates.iter().enumerate() {
        if let Some(policy) = &candidate.fresh_profile {
            for slot in &candidate.measurements {
                let repetition = slot.scheduling.comparison_block.measurement_index;
                if fresh_repetitions.contains(&(index, slot.specification.id, repetition)) {
                    store.register_fresh(
                        slot.candidate,
                        &slot.specification,
                        &slot.attempt_slot,
                        &slot.observation_slot,
                        crate::run_store::fresh::FreshPointRegistration {
                            measurement_index: repetition,
                            control: slot.fresh.as_ref().expect("fresh control"),
                            policy,
                        },
                    )?;
                }
            }
        }
    }
    let mut schedule = Vec::new();
    for &(candidate, measurement) in &resolved.schedule {
        let slot = &resolved.candidates[candidate].measurements[measurement];
        if !selected.contains(&(candidate, measurement)) {
            continue;
        }
        if cache_directory.is_some() {
            resolved.candidates[candidate]
                .dataset
                .artifact
                .add_root(&format!("run:{}", output_directory.display()))?;
        }
        if resolved.candidates[candidate].profile.is_some() {
            store.register_buffered(
                slot.candidate,
                &slot.specification,
                &slot.attempt_slot,
                &slot.observation_slots,
            )?;
        } else if resolved.candidates[candidate].fresh_profile.is_none() {
            store.register_one_shot(
                slot.candidate,
                &slot.specification,
                &slot.attempt_slot,
                &slot.observation_slot,
            )?;
        }
        let progress = store.slot_progress(slot.attempt_slot.id)?;
        if progress.accepted
            || progress.disposition.is_some()
            || (progress.attempts > 0 && !options.retry)
        {
            continue;
        }
        schedule.push((candidate, measurement));
    }
    selection::rebuild_batches(&mut resolved, schedule)?;
    let attempt_slots = resolved.schedule.len();
    let mut events = Vec::new();
    emit(
        &mut events,
        &mut event_sink,
        RunEventKind::RunStarted { attempt_slots },
    )?;
    let mut attempts = Vec::new();
    let mut workers = Vec::new();
    let mut busy_repetitions = std::collections::BTreeSet::new();
    for batch in &resolved.batches {
        workers.resize_with(
            batch.lane_count,
            crate::worker_reuse::ReusableWorker::default,
        );
        if options
            .max_attempts
            .is_some_and(|bound| attempts.len() as u64 >= bound)
        {
            break;
        }
        let mut slots = Vec::new();
        let mut executions = Vec::new();
        for &(candidate, measurement) in &resolved.schedule[batch.slots.clone()] {
            if options
                .max_attempts
                .is_some_and(|bound| (attempts.len() + executions.len()) as u64 >= bound)
            {
                break;
            }
            let slot = &resolved.candidates[candidate].measurements[measurement];
            let repetition = (
                slot.specification.id,
                slot.scheduling.comparison_block.measurement_index,
            );
            if slot.fresh.is_some() && busy_repetitions.contains(&repetition) {
                continue;
            }
            match AttemptExecution::reserve(&store, slot, options.retry) {
                Ok(Some(execution)) => {
                    slots.push((candidate, measurement));
                    executions.push(execution);
                }
                Ok(None) => (),
                Err(RunError::Store(crate::run_store::RunStoreError::SequenceBusy)) => {
                    // Stepping past an owned point could reorder this sequence if
                    // its owner finishes before our next batch.
                    busy_repetitions.insert(repetition);
                }
                Err(error) => return Err(error),
            }
        }
        for (lane, &(candidate, measurement)) in slots.iter().enumerate() {
            let provenance =
                &mut resolved.candidates[candidate].measurements[measurement].provenance;
            let mut value = provenance.as_json().clone();
            value["concurrency"]["batch_size"] = json!(slots.len());
            value["concurrency"]["lane_index"] = json!(lane);
            *provenance = canonical(value)?;
        }
        for &(candidate_index, measurement_index) in &slots {
            let measurement = &resolved.candidates[candidate_index].measurements[measurement_index];
            emit(
                &mut events,
                &mut event_sink,
                RunEventKind::AttemptStarted {
                    logical_slot_id: measurement.logical_slot,
                    resolved_slot_id: measurement.attempt_slot.id,
                },
            )?;
        }
        let completed = execute_batch(
            repository,
            &resolved.candidates,
            &slots,
            &mut executions,
            resolved.private_root.path(),
            &mut workers,
        )?;
        for attempt in completed {
            emit(
                &mut events,
                &mut event_sink,
                RunEventKind::AttemptFinished {
                    attempt: Box::new(attempt.clone()),
                },
            )?;
            attempts.push(attempt);
        }
    }
    let fresh_sequences = store.fresh_progress()?;
    let censored_profiles = fresh_sequences
        .iter()
        .filter(|p| p.status == "censored")
        .count();
    let attempt_slots = attempts.len();
    let censored_attempts = attempts
        .iter()
        .filter(|a| a.status == RunAttemptStatus::Censored)
        .count();
    let accepted_attempts = attempts
        .iter()
        .filter(|attempt| attempt.status == RunAttemptStatus::Accepted)
        .count();
    let failed_attempts = attempts
        .iter()
        .filter(|attempt| attempt.status == RunAttemptStatus::Failed)
        .count();
    let superseded_attempts = attempts
        .iter()
        .filter(|attempt| attempt.status == RunAttemptStatus::Superseded)
        .count();
    emit(
        &mut events,
        &mut event_sink,
        RunEventKind::RunFinished {
            censored_attempts,
            accepted_attempts,
            failed_attempts,
            superseded_attempts,
        },
    )?;
    Ok(RunReport {
        fresh_sequences,
        command: "run",
        benchmark: repository.manifest.name.to_string(),
        manifest_hash: repository.manifest_hash.to_string(),
        output_directory: output_directory
            .to_str()
            .expect("destination was checked for UTF-8")
            .to_owned(),
        attempts,
        events,
        summary: RunSummary {
            censored_profiles,
            censored_attempts,
            attempt_slots,
            accepted_attempts,
            failed_attempts,
            superseded_attempts,
        },
    })
}

fn execute_batch(
    repository: &CheckedRepository,
    candidates: &[ResolvedCandidate],
    slots: &[(usize, usize)],
    executions: &mut [AttemptExecution],
    private_root: &Path,
    workers: &mut [crate::worker_reuse::ReusableWorker],
) -> Result<Vec<RunAttemptOutput>, RunError> {
    if let [(candidate, measurement)] = slots {
        return execute(
            repository,
            &candidates[*candidate],
            &candidates[*candidate].measurements[*measurement],
            private_root,
            &mut workers[0],
            &mut executions[0],
        )
        .map(|attempt| vec![attempt]);
    }
    let results = thread::scope(|scope| {
        let mut handles = Vec::new();
        let mut spawn_error = None;
        for (lane, ((&(candidate, measurement), worker), execution)) in
            slots.iter().zip(workers).zip(executions).enumerate()
        {
            let candidate = &candidates[candidate];
            let measurement = &candidate.measurements[measurement];
            match thread::Builder::new()
                .name(format!("metewand-attempt-{lane}"))
                .spawn_scoped(scope, move || {
                    execute(
                        repository,
                        candidate,
                        measurement,
                        private_root,
                        worker,
                        execution,
                    )
                }) {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    spawn_error = Some(error);
                    break;
                }
            }
        }
        // Join every dispatched attempt before propagating an error or calling
        // the sink, so neither a failed peer nor completion order loses work.
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or(Err(RunError::ExecutionThreadPanic)))
            .collect::<Vec<_>>();
        match spawn_error {
            Some(error) => Err(RunError::Io(error)),
            None => Ok(results),
        }
    })?;
    results.into_iter().collect()
}

fn execute(
    repository: &CheckedRepository,
    candidate: &ResolvedCandidate,
    measurement: &ResolvedMeasurement,
    private_root: &Path,
    worker: &mut crate::worker_reuse::ReusableWorker,
    execution: &mut AttemptExecution,
) -> Result<RunAttemptOutput, RunError> {
    if candidate.profile.is_some() {
        return buffered::execute(
            repository,
            candidate,
            measurement,
            private_root,
            worker,
            execution,
        );
    }
    execution.advance(AttemptPhase::Materializing)?;
    let started_at = SystemTime::now();
    let mut measurements = ExecutionMeasurements {
        materialization_time_ns: candidate.dataset.materialization_time_ns,
        ..ExecutionMeasurements::default()
    };
    execution.advance(AttemptPhase::Starting)?;
    let executor =
        crate::executor::LocalProcessExecutor::new(std::time::Duration::from_millis(100));
    if let Err(error) = executor.preflight_policy(&candidate.reuse_key.policy) {
        worker.discard();
        return failed_attempt(
            measurement,
            execution,
            started_at,
            "control_preflight_failed",
            error.to_string(),
            &measurements,
        );
    }
    if let Err(error) = candidate.verify() {
        worker.discard();
        return failed_attempt(
            measurement,
            execution,
            started_at,
            input_change_code(&error),
            error.to_string(),
            &measurements,
        );
    }
    let dataset_view = match candidate.dataset.artifact.private_view(private_root) {
        Ok(view) => view,
        Err(error) => {
            return failed_attempt(
                measurement,
                execution,
                started_at,
                "dataset_mutation",
                error.to_string(),
                &measurements,
            );
        }
    };
    let effective_timeout = std::cell::Cell::new(candidate.timing.timeout);
    let request = || FixedDatasetRun {
        observation_control: measurement.fresh.as_ref().map(|f| &f.observation_control),
        dataset_dir: dataset_view.directory(),
        problem_contract_id: &candidate.contract_id,
        problem_parameters: &candidate.problem_parameters,
        problem_semantics: &candidate.problem_semantics,
        implementation_parameters: &candidate.implementation_parameters,
        implementation_seed: measurement.seed,
        implementation_launch: &candidate.implementation.launch,
        implementation_identity: &candidate.implementation.identity,
        evaluator_launch: &candidate.evaluator.launch,
        evaluator_identity: &candidate.evaluator.identity,
        result_schema: &candidate.result_schema,
        metric_schema: &candidate.metric_schema,
        schemas: &repository.schemas,
        private_root,
        timeouts: candidate.timeouts,
        timing: ExecutionTimingPolicy {
            timeout: effective_timeout.get(),
            ..candidate.timing
        },
        log_limits: WorkerLogLimits::new(64 * 1024, 64 * 1024),
    };
    let mut warmups = Vec::new();
    let startup = std::cell::Cell::new(None);
    let result = if candidate.reuse_key.policy.worker_reuse {
        use crate::worker_reuse::WorkerReuseError;
        let result = worker.run_slot_for_specification(
            candidate.reuse_key.clone(),
            measurement.attempt_slot.record.specification,
            || {
                let started = std::time::Instant::now();
                let launched = (|| {
                    let process = crate::local_worker::launch_trusted_local_worker(
                        &candidate.implementation.launch,
                        private_root,
                        request().log_limits,
                    )
                    .map_err(|source| FixedDatasetRunError::ImplementationLaunch { source })?;
                    crate::worker_session::WorkerSession::connect(
                        process,
                        metewand_protocol::WorkerRole::Implementation,
                        candidate.implementation.identity.clone(),
                        &candidate.reuse_key.selected_capabilities,
                        candidate.timeouts,
                    )
                    .map_err(|source| FixedDatasetRunError::Implementation { source })
                })();
                startup.set(Some(metewand_core::measurements::Measurement::elapsed(
                    started.elapsed(),
                    launched.is_ok(),
                )));
                launched
            },
            |session, index| {
                let mut measurements = ExecutionMeasurements::default();
                let result = run_fixed_dataset_with_session(
                    request(),
                    &dataset_view,
                    &mut measurements,
                    session,
                    None,
                );
                match &result {
                    Ok(result) => warmups.push(json!({
                        "index": index,
                        "resolved_slot_id": measurement.warmups[index as usize].1,
                        "implementation_seed": measurement.seed,
                        "status": "validated",
                        "result": result.result.as_json(), "metrics": result.metrics.as_json(),
                        "measurements": result.measurements,
                    })),
                    Err(error) => warmups.push(json!({
                        "index": index,
                        "resolved_slot_id": measurement.warmups[index as usize].1,
                        "implementation_seed": measurement.seed,
                        "status": "failed", "code": error.failure_kind().code(),
                        "message": error.to_string(), "measurements": measurements,
                    })),
                }
                result.map(|_| ())
            },
            |session| {
                if candidate.fresh_profile.is_some() {
                    effective_timeout.set(
                        execution
                            .reserve_fresh_time(candidate.timing.timeout)
                            .map_err(|error| {
                                FixedDatasetRunError::PrivateDirectory(io::Error::other(
                                    error.to_string(),
                                ))
                            })?,
                    );
                }
                run_fixed_dataset_with_session(
                    request(),
                    &dataset_view,
                    &mut measurements,
                    session,
                    Some(&mut |phase| execution.advance(phase)),
                )
            },
        );
        result.map_err(|error| match error {
            WorkerReuseError::Launch { source }
            | WorkerReuseError::Slot { source }
            | WorkerReuseError::Warmup { source, .. } => source,
            WorkerReuseError::Session(source) => FixedDatasetRunError::Implementation { source },
            WorkerReuseError::Cleanup(source) => FixedDatasetRunError::PrivateDirectory(source),
            error => FixedDatasetRunError::PrivateDirectory(io::Error::other(error.to_string())),
        })
    } else {
        if candidate.fresh_profile.is_some() {
            effective_timeout.set(execution.reserve_fresh_time(candidate.timing.timeout)?);
        }
        run_fixed_dataset_in_view(
            request(),
            &dataset_view,
            &mut measurements,
            Some(&mut |phase| execution.advance(phase)),
        )
    };
    if candidate.fresh_profile.is_some() {
        execution.record_fresh_time(&measurements)?;
    }
    let mut measurement = measurement.clone();
    let mut context = measurement.provenance.as_json().clone();
    if let (Some(policy), Some(fresh)) = (&candidate.fresh_profile, &measurement.fresh) {
        context["fresh_sequence"] = json!({"policy":policy,"measurement_index":measurement.scheduling.comparison_block.measurement_index,"control":fresh});
    }
    context["worker_reuse"] = json!({
        "enabled": candidate.reuse_key.policy.worker_reuse,
        "worker_startup_time_ns": startup.get(),
        "filesystem_cache": "clear_all_between_slots",
        "warmups_on_this_attempt": warmups,
    });
    if let Err(
        FixedDatasetRunError::Implementation {
            source: crate::worker_session::WorkerSessionError::ControlEnforcementLost { control },
        }
        | FixedDatasetRunError::Evaluator {
            source: crate::worker_session::WorkerSessionError::ControlEnforcementLost { control },
        },
    ) = &result
    {
        if let Some(controls) = context["controls"].as_array_mut() {
            for record in controls {
                if record["name"].as_str() == Some(control) {
                    record["status"] = json!("unsupported");
                }
            }
        }
        context["control_failure"] =
            json!({"control": control, "code": "control_enforcement_lost"});
    }
    measurement.provenance = canonical(context)?;
    let measurement = &measurement;
    let inputs_verified = candidate.verify();
    if let Err(error) = inputs_verified
        && !matches!(&result, Err(FixedDatasetRunError::DatasetMutation { .. }))
    {
        worker.discard();
        return failed_attempt(
            measurement,
            execution,
            started_at,
            input_change_code(&error),
            error.to_string(),
            &measurements,
        );
    }
    let mut result = match result {
        Ok(result) => result,
        Err(error) => {
            worker.discard();
            return failed_attempt(
                measurement,
                execution,
                started_at,
                error.failure_kind().code(),
                error.to_string(),
                &measurements,
            );
        }
    };
    if let (Some(policy), Some(fresh)) = (&candidate.fresh_profile, &measurement.fresh) {
        result.completion =
            if let (Some(metric), Some(threshold)) = (&policy.metric, policy.threshold) {
                let Some(value) = result
                    .metrics
                    .as_json()
                    .get(metric)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                else {
                    worker.discard();
                    return failed_attempt(
                        measurement,
                        execution,
                        started_at,
                        "invalid_metrics",
                        "The evaluator did not return a finite numeric target metric.".into(),
                        &measurements,
                    );
                };
                if policy.reaches(value, threshold) {
                    metewand_core::records::CompletionStatus::Reached
                } else {
                    metewand_core::records::CompletionStatus::NotReached
                }
            } else {
                metewand_core::records::CompletionStatus::NotRequired
            };
        result.execution_provenance["profile_observation"] =
            serde_json::to_value(metewand_core::profile::FreshObservationRecord {
                delivery: metewand_core::profile::FreshDelivery::FreshSequence,
                measurement_basis: metewand_core::profile::ExecutorBasis::ExecutorObserved,
                measurement_index: measurement.scheduling.comparison_block.measurement_index,
                schedule_index: fresh.schedule_index,
                observation_control: fresh.observation_control.clone(),
            })
            .expect("fresh observation evidence");
    }
    result.execution_provenance["controls"] = json!(metewand_core::controls::local_control_report(
        &candidate.reuse_key.policy
    ));
    let output_root = execution.output_root();
    let retry_index = execution.retry_index();
    let (published, attempt_directory, observation_directory) = execution.publish_result(
        &result,
        PublicationRequest {
            root: &output_root,
            attempt_slot: measurement.attempt_slot.id,
            observation_slot: measurement.observation_slot.id,
            retry_index,
            provenance_data: measurement.provenance.clone(),
        },
    )?;
    let (status, diagnostic) = match &published.attempt.record.outcome {
        RunAttemptOutcome::Buffered { .. } => unreachable!("single-result publisher"),
        RunAttemptOutcome::Censored { .. } => unreachable!("complete result publication"),
        RunAttemptOutcome::Accepted { .. } => (RunAttemptStatus::Accepted, None),
        RunAttemptOutcome::Superseded { .. } => (RunAttemptStatus::Superseded, None),
        RunAttemptOutcome::Failed { diagnostic, .. } => (
            RunAttemptStatus::Failed,
            Some(RunDiagnostic {
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
            }),
        ),
    };
    Ok(RunAttemptOutput {
        observations: vec![],
        logical_slot_id: measurement.logical_slot,
        resolved_slot_id: measurement.attempt_slot.id,
        attempt_id: published.attempt.id,
        status,
        observation_id: Some(published.observation.id),
        attempt_directory,
        observation_directory: Some(observation_directory),
        result: Some(result.result.as_json().clone()),
        metrics: Some(result.metrics.as_json().clone()),
        diagnostic,
    })
}

fn input_change_code(error: &RunError) -> &'static str {
    match error {
        RunError::ArtifactStore(_) => "dataset_mutation",
        _ => "input_changed",
    }
}

fn failed_attempt(
    measurement: &ResolvedMeasurement,
    execution: &mut AttemptExecution,
    started_at: SystemTime,
    code: &str,
    message: String,
    measurements: &ExecutionMeasurements,
) -> Result<RunAttemptOutput, RunError> {
    let diagnostic = RunDiagnostic {
        code: code.to_owned(),
        message: message.clone(),
    };
    let censored = measurement.fresh.is_some()
        && code == "scientific_timeout"
        && execution.fresh_budget_exhausted()?;
    let record = RunAttemptRecord {
        slot: measurement.attempt_slot.id,
        retry_index: execution.retry_index(),
        outcome: if censored {
            RunAttemptOutcome::Censored {
                reason: "active_time_limit".into(),
                observation: None,
            }
        } else {
            RunAttemptOutcome::Failed {
                diagnostic: AttemptDiagnostic {
                    code: code.to_owned(),
                    message,
                    details: None,
                },
                observation: None,
            }
        },
        started_at,
        finished_at: SystemTime::now(),
        provenance: ProvenanceRecord {
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            wire_protocol_version: metewand_protocol::WIRE_PROTOCOL_VERSION,
            execution_semantics_version: EXECUTION_SEMANTICS_VERSION,
            data: canonical(
                json!({"context": measurement.provenance.as_json(), "logs_available": false, "measurements": measurements}),
            )?,
        },
    };
    let attempt = metewand_core::identity::identify_record(record)?;
    let attempt_directory = execution.publish_failure(&attempt.record)?;
    Ok(RunAttemptOutput {
        observations: vec![],
        logical_slot_id: measurement.logical_slot,
        resolved_slot_id: measurement.attempt_slot.id,
        attempt_id: attempt.id,
        status: if censored {
            RunAttemptStatus::Censored
        } else {
            RunAttemptStatus::Failed
        },
        observation_id: None,
        attempt_directory,
        observation_directory: None,
        result: None,
        metrics: None,
        diagnostic: Some(diagnostic),
    })
}

fn emit(
    events: &mut Vec<RunEvent>,
    sink: &mut impl FnMut(&RunEvent) -> io::Result<()>,
    kind: RunEventKind,
) -> Result<(), RunError> {
    let event = RunEvent {
        command: "run",
        sequence: events.len(),
        kind,
    };
    sink(&event).map_err(RunError::Event)?;
    events.push(event);
    Ok(())
}

fn destination(path: &Path, resume: bool) -> Result<PathBuf, RunError> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "output directory must name a new child directory",
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let destination = fs::canonicalize(parent)?.join(name);
    if destination.to_str().is_none() {
        return Err(RunError::Unsupported(
            "output directory must be UTF-8".to_owned(),
        ));
    }
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if resume && metadata.is_dir() => {
            let database = destination.join("run.sqlite3");
            if !fs::symlink_metadata(&database).is_ok_and(|metadata| metadata.is_file()) {
                return Err(RunError::Selection(
                    "resume requires an existing durable run store".into(),
                ));
            }
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "output directory already exists; choose a new `--output-dir` path",
            )
            .into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && !resume => {}
        Err(error) => return Err(error.into()),
    }
    Ok(destination)
}

/// A failure outside an individual worker's terminal outcome.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunError {
    /// Applicability worker launch failed before measured execution.
    #[error(transparent)]
    ApplicabilityLaunch(#[from] crate::local_worker::LocalWorkerLaunchError),
    /// Applicability could not be determined.
    #[error(transparent)]
    ApplicabilityCheck(#[from] crate::worker_session::WorkerSessionError),
    /// Invalid execution selection or bound.
    #[error("{0}")]
    Selection(String),
    /// Durable coordination or publication failed.
    #[error(transparent)]
    Store(#[from] crate::run_store::RunStoreError),
    /// An internal attempt execution thread panicked after dispatch.
    #[error("an attempt execution thread panicked")]
    ExecutionThreadPanic,
    /// Artifact storage, construction mapping, or private-view creation failed.
    #[error(transparent)]
    ArtifactStore(#[from] crate::artifact_store::ArtifactStoreError),
    /// Dataset construction or contract validation failed.
    #[error(transparent)]
    Materialization(#[from] crate::dataset_materialization::MaterializationError),
    /// The selected manifest requires a later execution gate.
    #[error("{0}")]
    Unsupported(String),
    /// An input changed after repository validation or resolution.
    #[error("declared execution inputs changed after validation or during execution")]
    ChangedInput,
    /// Local resolution or output-directory setup failed.
    #[error("run setup failed: {0}")]
    Io(#[from] io::Error),
    /// The event consumer stopped accepting output.
    #[error("failed to deliver run event: {0}")]
    Event(#[source] io::Error),
    /// A canonical identity could not be constructed.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// A resolved value is outside the canonical JSON domain.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// Declared input expansion failed.
    #[error(transparent)]
    Sources(#[from] SourceBundleError),
    /// Declared input hashing failed.
    #[error(transparent)]
    SourceIdentity(#[from] SourceBundleIdentityError),
    /// Source acquisition or independent artifact verification failed.
    #[error(transparent)]
    SourceResolution(#[from] SourceResolutionError),
    /// A private input tree could not be hashed.
    #[error(transparent)]
    Tree(#[from] LocalTreeHashError),
    /// The embedded validation contracts could not be loaded.
    #[error(transparent)]
    PublicSchemas(#[from] PublicSchemaCatalogError),
    /// A fixed dataset does not satisfy its output schema.
    #[error(transparent)]
    Dataset(#[from] WorkerOutputError),
    /// An observation or attempt could not be durably finalized.
    #[error(transparent)]
    Publication(#[from] PublicationError),
}
