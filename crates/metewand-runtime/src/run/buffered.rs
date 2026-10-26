use super::*;
use crate::fixed_dataset_run::{BufferedEvent, run_buffered};
use metewand_core::{
    records::{CompletionStatus, ProfileAttemptStatus},
    run_output::ProfileObservationOutput,
};
use serde_json::Value;

pub(super) fn execute(
    repository: &CheckedRepository,
    candidate: &ResolvedCandidate,
    measurement: &ResolvedMeasurement,
    private_root: &Path,
    worker: &mut crate::worker_reuse::ReusableWorker,
    execution: &mut AttemptExecution,
) -> Result<RunAttemptOutput, RunError> {
    let profile = candidate.profile.as_ref().expect("buffered candidate");
    execution.advance(AttemptPhase::Materializing)?;
    let started_at = SystemTime::now();
    let mut measurements = ExecutionMeasurements {
        materialization_time_ns: candidate.dataset.materialization_time_ns,
        ..Default::default()
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
    let view = match candidate.dataset.artifact.private_view(private_root) {
        Ok(view) => view,
        Err(error) => {
            worker.discard();
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
    let request = || FixedDatasetRun {
        observation_control: None,
        dataset_dir: view.directory(),
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
        timing: candidate.timing,
        log_limits: WorkerLogLimits::new(64 * 1024, 64 * 1024),
    };
    let mut observations = Vec::new();
    let mut buffer_evidence = Value::Null;
    let mut target_reached = false;
    let mut stop_reason = "unreturned_buffer".to_owned();
    let mut expected_snapshots = 0;
    let mut warmups = Vec::new();
    let retry_index = execution.retry_index();
    let root = execution.output_root();
    let publication_error = |error: RunError| match error {
        RunError::Store(error) => FixedDatasetRunError::Coordination(error),
        error => FixedDatasetRunError::PrivateDirectory(io::Error::other(error.to_string())),
    };
    let mut event = |event: BufferedEvent<'_>| -> Result<(), FixedDatasetRunError> {
        match event {
            BufferedEvent::Phase(phase) => execution.advance(phase)?,
            BufferedEvent::Buffer {
                directory,
                manifest,
            } => {
                let artifact = execution
                    .publish_buffer(directory)
                    .map_err(&publication_error)?;
                expected_snapshots = manifest.snapshots.len();
                stop_reason = serde_json::to_value(&manifest.stop_reason)
                    .expect("stop reason")
                    .as_str()
                    .expect("stop text")
                    .into();
                buffer_evidence = json!({"artifact":artifact,"manifest":manifest});
            }
            BufferedEvent::Observation(result) => {
                result.execution_provenance["controls"] = json!(
                    metewand_core::controls::local_control_report(&candidate.reuse_key.policy)
                );
                let ordinal = observations.len();
                let (published, directory) = execution
                    .publish_profile_observation(
                        result,
                        PublicationRequest {
                            root: &root,
                            attempt_slot: measurement.attempt_slot.id,
                            observation_slot: measurement.observation_slots[ordinal].id,
                            retry_index,
                            provenance_data: measurement.provenance.clone(),
                        },
                    )
                    .map_err(&publication_error)?;
                target_reached |= result.completion == CompletionStatus::Reached;
                observations.push(ProfileObservationOutput {
                    observation_id: published.id,
                    observation_directory: directory,
                    result: result.result.as_json().clone(),
                    metrics: result.metrics.as_json().clone(),
                    profile: published.record.profile.expect("buffered observation"),
                });
            }
        }
        Ok(())
    };
    let startup = std::cell::Cell::new(None);
    let result = if candidate.reuse_key.policy.worker_reuse {
        use crate::worker_reuse::WorkerReuseError;
        worker
            .run_slot_for_specification(
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
                            &[metewand_protocol::Capability::BufferedProfile],
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
                    let mut times = ExecutionMeasurements::default();
                    let result = run_buffered(
                        request(),
                        profile,
                        &view,
                        &mut times,
                        Some(session),
                        &mut |_| Ok(()),
                    );
                    warmups.push(json!({
                        "index": index,
                        "resolved_slot_id": measurement.warmups[index as usize].1,
                        "implementation_seed": measurement.seed,
                        "measurements": times,
                        "status": if result.is_ok() { "validated" } else { "failed" },
                        "diagnostic": result.as_ref().err().map(|error| json!({
                            "code": error.failure_kind().code(), "message": error.to_string(),
                        })),
                    }));
                    result
                },
                |session| {
                    run_buffered(
                        request(),
                        profile,
                        &view,
                        &mut measurements,
                        Some(session),
                        &mut event,
                    )
                },
            )
            .map_err(|error| match error {
                WorkerReuseError::Launch { source }
                | WorkerReuseError::Slot { source }
                | WorkerReuseError::Warmup { source, .. } => source,
                WorkerReuseError::Session(source) => {
                    FixedDatasetRunError::Implementation { source }
                }
                WorkerReuseError::Cleanup(source) => FixedDatasetRunError::PrivateDirectory(source),
                error => {
                    FixedDatasetRunError::PrivateDirectory(io::Error::other(error.to_string()))
                }
            })
    } else {
        run_buffered(
            request(),
            profile,
            &view,
            &mut measurements,
            None,
            &mut event,
        )
    };
    let result = match candidate.verify() {
        Ok(()) => result,
        Err(error) => Err(FixedDatasetRunError::InvalidBuffer(error.to_string())),
    };
    let (status, reason, diagnostic) = match result {
        Err(error) => {
            worker.discard();
            let code = error.failure_kind().code().to_owned();
            let reason = error.to_string();
            (
                ProfileAttemptStatus::Failed,
                reason.clone(),
                Some(AttemptDiagnostic {
                    code,
                    message: reason,
                    details: None,
                }),
            )
        }
        Ok(())
            if observations.len() == expected_snapshots
                && (target_reached
                    || (profile.policy.metric.is_none()
                        && observations.len() as u64 == profile.policy.max_observations)) =>
        {
            (ProfileAttemptStatus::Accepted, stop_reason, None)
        }
        Ok(()) => (
            ProfileAttemptStatus::Censored,
            if stop_reason == "target_reached" {
                "local_target_unconfirmed".into()
            } else {
                stop_reason
            },
            None,
        ),
    };
    let provenance = ProvenanceRecord {
        tool_version: env!("CARGO_PKG_VERSION").into(),
        wire_protocol_version: metewand_protocol::WIRE_PROTOCOL_VERSION,
        execution_semantics_version: EXECUTION_SEMANTICS_VERSION,
        data: canonical(
            json!({"context":measurement.provenance.as_json(),"measurements":measurements,"profile":{"selected":profile,"buffer":buffer_evidence,"worker_reuse":candidate.reuse_key.policy.worker_reuse,"worker_startup_time_ns":startup.get(),"warmups":warmups}}),
        )?,
    };
    let mut record = RunAttemptRecord {
        slot: measurement.attempt_slot.id,
        retry_index,
        outcome: RunAttemptOutcome::Buffered {
            status,
            observations: observations.iter().map(|o| o.observation_id).collect(),
            reason: reason.clone(),
            diagnostic: diagnostic.clone(),
        },
        started_at,
        finished_at: SystemTime::now(),
        provenance,
    };
    let attempt_directory = execution.publish_buffered(&mut record)?;
    let attempt = metewand_core::identity::identify_record(record)?;
    let RunAttemptOutcome::Buffered { status, .. } = attempt.record.outcome else {
        unreachable!()
    };
    let status = match status {
        ProfileAttemptStatus::Accepted => RunAttemptStatus::Accepted,
        ProfileAttemptStatus::Censored => RunAttemptStatus::Censored,
        ProfileAttemptStatus::Superseded => RunAttemptStatus::Superseded,
        ProfileAttemptStatus::Failed => RunAttemptStatus::Failed,
    };
    Ok(RunAttemptOutput {
        observations,
        logical_slot_id: measurement.logical_slot,
        resolved_slot_id: measurement.attempt_slot.id,
        attempt_id: attempt.id,
        status,
        observation_id: None,
        attempt_directory,
        observation_directory: None,
        result: None,
        metrics: None,
        diagnostic: diagnostic
            .map(|d| RunDiagnostic {
                code: d.code,
                message: d.message,
            })
            .or_else(|| {
                (status == RunAttemptStatus::Censored).then(|| RunDiagnostic {
                    code: reason.clone(),
                    message: format!("Profile was censored: {reason}."),
                })
            }),
    })
}
