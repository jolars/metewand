use super::*;
use crate::core::{
    records::{CompletionStatus, ProfileAttemptStatus},
    run_output::ProfileObservationOutput,
};
use crate::runtime::fixed_dataset_run::{ProfileEvent, run_buffered, run_streaming};
use serde_json::Value;

pub(super) fn execute(
    repository: &CheckedRepository,
    candidate: &ResolvedCandidate,
    measurement: &ResolvedMeasurement,
    private_root: &Path,
    worker: &mut crate::runtime::worker_reuse::ReusableWorker,
    execution: &mut AttemptExecution,
) -> Result<RunAttemptOutput, RunError> {
    let policy = candidate
        .profile
        .as_ref()
        .map(|p| &p.policy)
        .or_else(|| candidate.streaming_profile.as_ref().map(|p| &p.policy))
        .expect("profile candidate");
    let profile_value = if let Some(profile) = &candidate.profile {
        serde_json::to_value(profile)
    } else {
        serde_json::to_value(&candidate.streaming_profile)
    }
    .expect("selected profile");
    execution.advance(AttemptPhase::Materializing)?;
    let started_at = SystemTime::now();
    let mut measurements = ExecutionMeasurements {
        materialization_time_ns: candidate.dataset.materialization_time_ns,
        ..Default::default()
    };
    execution.advance(AttemptPhase::Starting)?;
    let executor =
        crate::runtime::executor::LocalProcessExecutor::new(std::time::Duration::from_millis(100));
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
    let problem_view = match candidate
        .preparation
        .as_ref()
        .map(|preparation| preparation.artifact.private_view(private_root))
        .transpose()
    {
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
        problem_view: problem_view.as_ref(),
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
    let observation_slots = measurement.resolve_observations()?;
    let mut observations = Vec::new();
    let mut published_buffer = None;
    let mut buffer_evidence = Value::Null;
    let mut target_reached = false;
    let mut late_target = false;
    let adaptive = measurement.provenance().as_json()["adaptive_budget"].is_object()
        || measurement.specification.record.calibration.is_some();
    let mut stop_reason = "unreturned_buffer".to_owned();
    let mut expected_snapshots = 0;
    let mut warmups = Vec::new();
    let retry_index = execution.retry_index();
    let root = execution.output_root();
    let publication_error = |error: RunError| match error {
        RunError::Store(error) => FixedDatasetRunError::Coordination(error),
        error => FixedDatasetRunError::PrivateDirectory(io::Error::other(error.to_string())),
    };
    let mut event = |event: ProfileEvent<'_>| -> Result<(), FixedDatasetRunError> {
        match event {
            ProfileEvent::Phase(phase) => execution.advance(phase)?,
            ProfileEvent::Stopped(reason) => {
                stop_reason = serde_json::to_value(reason)
                    .expect("reason")
                    .as_str()
                    .expect("text")
                    .into();
            }
            ProfileEvent::Buffer {
                directory,
                manifest,
                digest,
            } => {
                let artifact = execution
                    .publish_buffer(directory, digest)
                    .map_err(&publication_error)?;
                expected_snapshots = manifest.snapshots.len();
                stop_reason = serde_json::to_value(&manifest.stop_reason)
                    .expect("stop reason")
                    .as_str()
                    .expect("stop text")
                    .into();
                buffer_evidence = json!({
                    "artifact": {"id": artifact.id, "directory": artifact.relative_path},
                    "manifest": manifest
                });
                published_buffer = Some(root.join(artifact.relative_path));
            }
            ProfileEvent::Observation {
                result,
                buffered_snapshot,
            } => {
                let published_snapshot = buffered_snapshot.map(|relative| {
                    published_buffer
                        .as_ref()
                        .expect("buffer published before evaluation")
                        .join(relative)
                });
                result.execution_provenance["environment_class"] =
                    json!(candidate.environment_class());
                result.execution_provenance["controls"] =
                    json!(if candidate.streaming_profile.is_some() {
                        crate::runtime::resource_accounting::streaming_controls(
                            &candidate.reuse_key.policy,
                            result.measurements.cpu_accounting.as_deref(),
                        )
                    } else {
                        crate::core::controls::local_control_report(&candidate.reuse_key.policy)
                    });
                let ordinal = observations.len();
                let observation = execution
                    .publish_profile_observation(
                        result,
                        PublicationRequest {
                            primary_time: candidate.reuse_key.policy.primary_time,
                            root: &root,
                            attempt_slot: measurement.attempt_slot,
                            observation_slot: observation_slots[ordinal].id,
                            retry_index,
                            provenance_data: measurement.provenance().clone(),
                        },
                        published_snapshot.as_deref(),
                    )
                    .map_err(&publication_error)?;
                let clock = if candidate.streaming_profile.is_some() {
                    "active_time_ns"
                } else {
                    "instrumented_time_ns"
                };
                let within_budget = !adaptive
                    || result.execution_provenance["profile_observation"][clock]
                        .as_u64()
                        .is_some_and(|time| {
                            time <= policy.active_time_ns().expect("validated policy")
                        });
                target_reached |= result.completion == CompletionStatus::Reached && within_budget;
                late_target |= result.completion == CompletionStatus::Reached && !within_budget;
                observations.push(ProfileObservationOutput {
                    observation_id: observation.record.id,
                    observation_directory: observation.directory,
                    result: observation.result,
                    metrics: observation.metrics,
                    profile: observation
                        .record
                        .record
                        .profile
                        .expect("profile observation"),
                });
            }
        }
        Ok(())
    };
    let startup = std::cell::Cell::new(None);
    let result = if candidate.reuse_key.policy.worker_reuse {
        use crate::runtime::worker_reuse::WorkerReuseError;
        worker
            .run_slot_for_specification(
                candidate.reuse_key.clone(),
                measurement.specification.id,
                || {
                    let started = std::time::Instant::now();
                    let launched = (|| {
                        let process = crate::runtime::local_worker::launch_trusted_local_worker(
                            &candidate.implementation.launch,
                            private_root,
                            request().log_limits,
                        )
                        .map_err(|source| FixedDatasetRunError::ImplementationLaunch { source })?;
                        crate::runtime::worker_session::WorkerSession::connect(
                            process,
                            crate::protocol::WorkerRole::Implementation,
                            candidate.implementation.identity.clone(),
                            &candidate.reuse_key.selected_capabilities,
                            candidate.timeouts,
                        )
                        .map_err(|source| FixedDatasetRunError::Implementation { source })
                    })();
                    startup.set(Some(crate::core::measurements::Measurement::elapsed(
                        started.elapsed(),
                        launched.is_ok(),
                    )));
                    launched
                },
                |session, index| {
                    let mut times = ExecutionMeasurements::default();
                    let result = if adaptive && let Some(stream) = &candidate.streaming_profile {
                        let mut stream = stream.clone();
                        let mut warmup = request();
                        let watchdog = candidate.timeouts.execute;
                        stream.policy.max_active_time = format!("{} ms", watchdog.as_millis());
                        warmup.timing.timeout = Some(watchdog);
                        run_streaming(
                            warmup,
                            &stream,
                            &view,
                            &mut times,
                            Some(session),
                            &mut |_| Ok(()),
                            true,
                        )
                    } else {
                        run_profile(
                            request(),
                            candidate,
                            &view,
                            &mut times,
                            Some(session),
                            &mut |_| Ok(()),
                            adaptive,
                        )
                    };
                    warmups.push(json!({
                        "index": index,
                        "resolved_slot_id": measurement.warmups[index as usize].id,
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
                    run_profile(
                        request(),
                        candidate,
                        &view,
                        &mut measurements,
                        Some(session),
                        &mut event,
                        adaptive,
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
        run_profile(
            request(),
            candidate,
            &view,
            &mut measurements,
            None,
            &mut event,
            adaptive,
        )
    };
    let result = match candidate.verify() {
        Ok(()) => result,
        Err(error) => Err(FixedDatasetRunError::InvalidBuffer(error.to_string())),
    };
    let (status, reason, diagnostic) = match result {
        Err(error)
            if candidate.streaming_profile.is_some()
                && (!adaptive || !warmups.last().is_some_and(|warmup| warmup["status"] == "failed"))
                && error.failure_kind()
                    == crate::runtime::fixed_dataset_run::FixedDatasetFailureKind::ScientificTimeout =>
        {
            worker.discard();
            (
                ProfileAttemptStatus::Censored,
                "active_time_limit".into(),
                None,
            )
        }

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
            if (candidate.streaming_profile.is_some()
                || observations.len() == expected_snapshots)
                && (target_reached
                    || (policy.metric.is_none()
                        && observations.len() as u64 == policy.max_observations)) =>
        {
            (ProfileAttemptStatus::Accepted, stop_reason, None)
        }
        Ok(()) => (
            ProfileAttemptStatus::Censored,
            if late_target {
                "active_time_limit".into()
            } else if stop_reason == "target_reached" {
                "local_target_unconfirmed".into()
            } else {
                stop_reason
            },
            None,
        ),
    };
    if let Some(stream) = &candidate.streaming_profile {
        buffer_evidence = json!({"delivery":"streaming_profile","policy":stream.policy});
    }
    let provenance = ProvenanceRecord {
        tool_version: env!("CARGO_PKG_VERSION").into(),
        wire_protocol_version: crate::protocol::WIRE_PROTOCOL_VERSION,
        execution_semantics_version: EXECUTION_SEMANTICS_VERSION,
        data: canonical(
            json!({"context":measurement.provenance().as_json(),"measurements":measurements,"storage":crate::runtime::platform_fs::storage_evidence(),"profile":{"selected":profile_value,"buffer":buffer_evidence,"worker_reuse":candidate.reuse_key.policy.worker_reuse,"worker_startup_time_ns":startup.get(),"warmups":warmups}}),
        )?,
    };
    let mut record = RunAttemptRecord {
        slot: measurement.attempt_slot,
        retry_index,
        outcome: RunAttemptOutcome::Profile {
            status,
            observations: observations.iter().map(|o| o.observation_id).collect(),
            reason: reason.clone(),
            diagnostic: diagnostic.clone(),
        },
        started_at,
        finished_at: SystemTime::now(),
        provenance,
    };
    let attempt_directory = execution.publish_profile(&mut record)?;
    let attempt = crate::core::identity::identify_record(record)?;
    let RunAttemptOutcome::Profile { status, .. } = attempt.record.outcome else {
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
        resolved_slot_id: measurement.attempt_slot,
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

fn run_profile(
    request: FixedDatasetRun<'_>,
    candidate: &ResolvedCandidate,
    view: &crate::runtime::artifact_store::DatasetView,
    measurements: &mut ExecutionMeasurements,
    session: Option<&mut crate::runtime::worker_session::WorkerSession>,
    event: &mut dyn FnMut(ProfileEvent<'_>) -> Result<(), FixedDatasetRunError>,
    adaptive: bool,
) -> Result<(), FixedDatasetRunError> {
    if let Some(profile) = &candidate.streaming_profile {
        run_streaming(
            request,
            profile,
            view,
            measurements,
            session,
            event,
            adaptive,
        )
    } else {
        run_buffered(
            request,
            candidate.profile.as_ref().expect("buffered profile"),
            view,
            measurements,
            session,
            event,
        )
    }
}
