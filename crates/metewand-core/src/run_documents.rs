//! Public, canonical documents rendered from typed execution records.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    PUBLIC_SCHEMA_VERSION,
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, IdentityKind, record_id},
    manifest::PrimaryTime,
    records::{
        ArtifactReference, CompletionStatus, ProvenanceRecord, RunAttemptOutcome, RunAttemptRecord,
        RunObservationRecord,
    },
};

/// Renders the versioned public observation envelope.
///
/// # Errors
///
/// Rejects values outside the public canonical JSON domain.
pub fn observation_document(
    record: &RunObservationRecord,
) -> Result<CanonicalValue, RunDocumentError> {
    if record.timing.primary_time != PrimaryTime::TimedWallTime {
        return Err(RunDocumentError::UnsupportedPrimaryTime);
    }
    let mut value = json!({
        "schema_version": PUBLIC_SCHEMA_VERSION,
        "id": record_id(record)?,
        "slot_id": record.slot,
        "attempt_id": record.producing_attempt,
        "result": artifact(&record.result),
        "metrics": artifact(&record.metrics),
        "timing": {
            "primary_time": "timed_wall_time",
            "timed_wall_time_ns": nanoseconds(record.timing.timed_wall_time)?,
            "implementation_time_ns": record.timing.implementation_time.map(nanoseconds).transpose()?,
            "evaluation_time_ns": nanoseconds(record.timing.evaluation_time)?,
            "cpu_time_ns": record.timing.cpu_time.map(nanoseconds).transpose()?,
            "peak_rss_bytes": record.timing.peak_rss_bytes,
        },
        "validity": {"valid": true, "completion": match record.completion {
            CompletionStatus::NotRequired => "not_required",
            CompletionStatus::Reached => "reached",
            CompletionStatus::NotReached => "not_reached",
        }},
        "provenance": provenance(&record.provenance),
    });
    if let Some(profile) = &record.profile {
        value["profile"] = serde_json::to_value(profile).expect("profile evidence");
    }
    Ok(CanonicalValue::try_from(value)?)
}

/// Renders the versioned public terminal attempt envelope.
///
/// # Errors
///
/// Rejects unsupported timestamps and values outside canonical JSON.
pub fn attempt_document(record: &RunAttemptRecord) -> Result<CanonicalValue, RunDocumentError> {
    if let RunAttemptOutcome::Buffered {
        status,
        observations,
        reason,
        diagnostic,
    } = &record.outcome
    {
        return Ok(CanonicalValue::try_from(
            json!({"schema_version":PUBLIC_SCHEMA_VERSION,"id":record_id(record)?,"slot_id":record.slot,"retry_index":record.retry_index,
            "outcome":{"kind":status,"reason":reason,"diagnostic":diagnostic.as_ref().map(|d|json!({"code":d.code,"message":d.message,"details":d.details.as_ref().map(CanonicalValue::as_json)}))},
            "observations":observations,"started_at":timestamp(record.started_at)?,"finished_at":timestamp(record.finished_at)?,"provenance":provenance(&record.provenance)}),
        )?);
    }
    let (outcome, observation) = match &record.outcome {
        RunAttemptOutcome::Buffered { .. } => unreachable!("handled buffered outcome"),
        RunAttemptOutcome::Censored {
            reason,
            observation,
        } => (json!({"kind":"censored","reason":reason}), *observation),
        RunAttemptOutcome::Accepted { observation } => {
            (json!({"kind": "accepted"}), Some(*observation))
        }
        RunAttemptOutcome::Superseded { observation } => {
            (json!({"kind": "superseded"}), *observation)
        }
        RunAttemptOutcome::Failed {
            diagnostic,
            observation,
        } => {
            let mut value = json!({"code": diagnostic.code, "message": diagnostic.message});
            if let Some(details) = &diagnostic.details {
                value["details"] = details.as_json().clone();
            }
            (json!({"kind": "failed", "diagnostic": value}), *observation)
        }
    };
    Ok(CanonicalValue::try_from(json!({
        "schema_version": PUBLIC_SCHEMA_VERSION,
        "id": record_id(record)?,
        "slot_id": record.slot,
        "retry_index": record.retry_index,
        "outcome": outcome,
        "observations": observation.into_iter().collect::<Vec<_>>(),
        "started_at": timestamp(record.started_at)?,
        "finished_at": timestamp(record.finished_at)?,
        "provenance": provenance(&record.provenance),
    }))?)
}

fn artifact<T: IdentityKind>(artifact: &ArtifactReference<T>) -> Value {
    json!({
        "id": artifact.id,
        "path": artifact.path.as_path(),
        "sha256": artifact.sha256.bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
    })
}

fn provenance(record: &ProvenanceRecord) -> Value {
    json!({
        "tool_version": record.tool_version,
        "wire_protocol_version": record.wire_protocol_version,
        "execution_semantics_version": record.execution_semantics_version,
        "data": record.data.as_json(),
    })
}

fn nanoseconds(duration: Duration) -> Result<u64, RunDocumentError> {
    u64::try_from(duration.as_nanos()).map_err(|_| RunDocumentError::Duration)
}

fn timestamp(time: SystemTime) -> Result<String, RunDocumentError> {
    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RunDocumentError::Timestamp)?;
    let mut days = elapsed.as_secs() / 86400;
    let mut year = 1970;
    while year <= 9999 {
        let length = if leap_year(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    if year > 9999 {
        return Err(RunDocumentError::Timestamp);
    }
    let months = [
        31,
        if leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while days >= months[month] {
        days -= months[month];
        month += 1;
    }
    let seconds = elapsed.as_secs() % 86400;
    Ok(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        month + 1,
        days + 1,
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        elapsed.subsec_nanos()
    ))
}

fn leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// Failure to represent a typed execution record in the public JSON contract.
#[derive(Debug, Error)]
pub enum RunDocumentError {
    /// Gate 1 public observations support only wall time as primary time.
    #[error("Gate 1 observation documents require timed_wall_time")]
    UnsupportedPrimaryTime,
    /// A record contains a noncanonical value.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// A typed identity cannot be computed.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// A timestamp lies outside the supported RFC 3339 years.
    #[error("timestamp must be between 1970 and 9999 UTC")]
    Timestamp,
    /// A duration cannot be represented as integer nanoseconds.
    #[error("duration exceeds the public nanosecond representation")]
    Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_preserve_utc_dates_and_nanoseconds() {
        for (seconds, expected) in [
            (0, "1970-01-01T00:00:00.000000123Z"),
            (951782400, "2000-02-29T00:00:00.000000123Z"),
            (4107542400, "2100-03-01T00:00:00.000000123Z"),
            (253402300799, "9999-12-31T23:59:59.000000123Z"),
        ] {
            assert_eq!(
                timestamp(UNIX_EPOCH + Duration::new(seconds, 123)).unwrap(),
                expected
            );
        }
        assert!(timestamp(UNIX_EPOCH - Duration::from_secs(1)).is_err());
        assert!(timestamp(UNIX_EPOCH + Duration::from_secs(253402300800)).is_err());
    }
}
