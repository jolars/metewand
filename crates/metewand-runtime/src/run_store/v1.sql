-- Apply to a new database inside a transaction, with foreign_keys and
-- recursive_triggers enabled on the connection before beginning it.
PRAGMA user_version = 1;

CREATE TABLE applicability_decisions (
    id TEXT PRIMARY KEY NOT NULL,
    candidate_id TEXT NOT NULL,
    applicable INTEGER NOT NULL CHECK (applicable IN (0, 1)),
    record_json TEXT NOT NULL CHECK (json_valid(record_json) AND json_type(record_json) = 'object'),
    UNIQUE (id, candidate_id, applicable)
) STRICT;

CREATE TABLE resolved_runs (
    id TEXT PRIMARY KEY NOT NULL,
    logical_specification_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    applicability_id TEXT NOT NULL,
    applicable INTEGER NOT NULL DEFAULT 1 CHECK (applicable = 1),
    record_json TEXT NOT NULL CHECK (json_valid(record_json) AND json_type(record_json) = 'object'),
    FOREIGN KEY (applicability_id, candidate_id, applicable)
        REFERENCES applicability_decisions (id, candidate_id, applicable)
) STRICT;

CREATE TABLE observation_slots (
    id TEXT PRIMARY KEY NOT NULL,
    logical_slot_id TEXT NOT NULL,
    resolved_run_id TEXT NOT NULL REFERENCES resolved_runs (id),
    record_json TEXT NOT NULL CHECK (json_valid(record_json) AND json_type(record_json) = 'object'),
    UNIQUE (logical_slot_id, resolved_run_id),
    UNIQUE (id, resolved_run_id)
) STRICT;

CREATE TABLE attempt_slots (
    id TEXT PRIMARY KEY NOT NULL,
    logical_slot_id TEXT NOT NULL,
    resolved_run_id TEXT NOT NULL REFERENCES resolved_runs (id),
    kind TEXT NOT NULL CHECK (kind IN ('one_shot', 'fresh_observation', 'stream')),
    role TEXT NOT NULL CHECK (role IN ('measurement', 'warmup')),
    record_json TEXT NOT NULL CHECK (json_valid(record_json) AND json_type(record_json) = 'object'),
    UNIQUE (logical_slot_id, resolved_run_id),
    UNIQUE (id, resolved_run_id)
) STRICT;

-- Explicit membership supports several checkpoints from one stream while
-- preventing retries from attaching observations to another execution slot.
CREATE TABLE attempt_observation_slots (
    attempt_slot_id TEXT NOT NULL,
    observation_slot_id TEXT PRIMARY KEY NOT NULL,
    resolved_run_id TEXT NOT NULL,
    FOREIGN KEY (attempt_slot_id, resolved_run_id) REFERENCES attempt_slots (id, resolved_run_id),
    FOREIGN KEY (observation_slot_id, resolved_run_id) REFERENCES observation_slots (id, resolved_run_id),
    UNIQUE (attempt_slot_id, observation_slot_id)
) STRICT;

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('attempt', 'observation', 'result', 'metrics', 'dataset', 'source', 'environment', 'other')),
    relative_path TEXT NOT NULL UNIQUE CHECK (
        length(relative_path) > 0 AND substr(relative_path, 1, 1) != '/'
        AND instr(relative_path, char(0)) = 0 AND instr(relative_path, char(92)) = 0
        AND instr('/' || relative_path || '/', '//') = 0
        AND instr('/' || relative_path || '/', '/./') = 0
        AND instr('/' || relative_path || '/', '/../') = 0
    ),
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64 AND content_sha256 NOT GLOB '*[^0-9a-f]*'),
    storage_kind TEXT NOT NULL CHECK (storage_kind IN ('file', 'directory')),
    manifest_sha256 TEXT CHECK (manifest_sha256 IS NULL OR (length(manifest_sha256) = 64 AND manifest_sha256 NOT GLOB '*[^0-9a-f]*')),
    verified_at_ns INTEGER NOT NULL CHECK (verified_at_ns >= 0),
    CHECK (storage_kind != 'directory' OR manifest_sha256 IS NOT NULL),
    UNIQUE (id, kind)
) STRICT;

CREATE TABLE attempts (
    id TEXT PRIMARY KEY NOT NULL,
    slot_id TEXT NOT NULL REFERENCES attempt_slots (id),
    retry_index INTEGER NOT NULL CHECK (retry_index >= 0),
    state TEXT NOT NULL CHECK (state IN ('reserved', 'materializing', 'starting', 'preparing', 'executing', 'checkpointing', 'validating', 'evaluating', 'finalizing', 'accepted', 'censored', 'superseded', 'failed')),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    started_at_ns INTEGER NOT NULL CHECK (started_at_ns >= 0),
    updated_at_ns INTEGER NOT NULL CHECK (updated_at_ns >= started_at_ns),
    finished_at_ns INTEGER,
    failure_code TEXT CHECK (failure_code IS NULL OR length(failure_code) > 0),
    final_artifact_id TEXT,
    final_artifact_kind TEXT NOT NULL DEFAULT 'attempt' CHECK (final_artifact_kind = 'attempt'),
    CHECK ((state IN ('accepted', 'censored', 'superseded', 'failed')) = (finished_at_ns IS NOT NULL)),
    CHECK (finished_at_ns IS NULL OR finished_at_ns = updated_at_ns),
    CHECK ((state = 'failed') = (failure_code IS NOT NULL)),
    CHECK ((state IN ('accepted', 'censored', 'superseded', 'failed')) = (final_artifact_id IS NOT NULL)),
    UNIQUE (slot_id, retry_index),
    UNIQUE (id, slot_id),
    FOREIGN KEY (final_artifact_id, final_artifact_kind) REFERENCES artifacts (id, kind),
    FOREIGN KEY (id, revision, state, updated_at_ns)
        REFERENCES attempt_transitions (attempt_id, revision, to_state, occurred_at_ns)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE UNIQUE INDEX one_accepted_attempt_per_slot ON attempts (slot_id) WHERE state = 'accepted';
CREATE INDEX attempts_by_state ON attempts (state, slot_id);

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id TEXT REFERENCES attempts (id),
    occurred_at_ns INTEGER NOT NULL CHECK (occurred_at_ns >= 0),
    kind TEXT NOT NULL CHECK (length(kind) > 0),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json) AND json_type(payload_json) = 'object'),
    UNIQUE (sequence, attempt_id, occurred_at_ns)
) STRICT;
CREATE INDEX events_by_attempt ON events (attempt_id, sequence);

-- The deferred projection reference lets reservation and each state update
-- commit together with their event and contiguous transition history.
CREATE TABLE attempt_transitions (
    attempt_id TEXT NOT NULL REFERENCES attempts (id),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    previous_revision INTEGER,
    from_state TEXT,
    to_state TEXT NOT NULL,
    occurred_at_ns INTEGER NOT NULL CHECK (occurred_at_ns >= 0),
    event_sequence INTEGER NOT NULL UNIQUE,
    PRIMARY KEY (attempt_id, revision),
    UNIQUE (attempt_id, revision, to_state),
    UNIQUE (attempt_id, revision, to_state, occurred_at_ns),
    FOREIGN KEY (attempt_id, previous_revision, from_state)
        REFERENCES attempt_transitions (attempt_id, revision, to_state),
    FOREIGN KEY (event_sequence, attempt_id, occurred_at_ns)
        REFERENCES events (sequence, attempt_id, occurred_at_ns),
    CHECK (
        (revision = 0 AND previous_revision IS NULL AND from_state IS NULL AND to_state = 'reserved')
        OR (revision > 0 AND previous_revision IS NOT NULL AND previous_revision = revision - 1 AND from_state IS NOT NULL AND (
            (from_state = 'reserved' AND to_state = 'materializing')
            OR (from_state = 'materializing' AND to_state = 'starting')
            OR (from_state = 'starting' AND to_state = 'preparing')
            OR (from_state = 'preparing' AND to_state = 'executing')
            OR (from_state = 'executing' AND to_state IN ('checkpointing', 'validating'))
            OR (from_state = 'checkpointing' AND to_state = 'validating')
            OR (from_state = 'validating' AND to_state = 'evaluating')
            OR (from_state = 'evaluating' AND to_state = 'executing')
            OR (from_state IN ('reserved', 'materializing', 'starting', 'preparing', 'executing', 'checkpointing', 'validating', 'evaluating') AND to_state = 'finalizing')
            OR (from_state = 'finalizing' AND to_state IN ('accepted', 'censored', 'superseded', 'failed'))
        ))
    )
) STRICT;

CREATE TABLE leases (
    id TEXT PRIMARY KEY NOT NULL,
    owner_token TEXT NOT NULL CHECK (length(owner_token) > 0),
    attempt_id TEXT REFERENCES attempts (id),
    purpose TEXT NOT NULL CHECK (purpose IN ('reservation', 'staging', 'finalization', 'artifact')),
    staging_path TEXT UNIQUE CHECK (staging_path IS NULL OR (
        length(staging_path) > 0 AND substr(staging_path, 1, 1) != '/'
        AND instr(staging_path, char(0)) = 0 AND instr(staging_path, char(92)) = 0
        AND instr('/' || staging_path || '/', '//') = 0
        AND instr('/' || staging_path || '/', '/./') = 0
        AND instr('/' || staging_path || '/', '/../') = 0
    )),
    acquired_at_ns INTEGER NOT NULL CHECK (acquired_at_ns >= 0),
    expires_at_ns INTEGER NOT NULL CHECK (expires_at_ns > acquired_at_ns),
    released_at_ns INTEGER CHECK (released_at_ns IS NULL OR released_at_ns >= acquired_at_ns),
    CHECK (purpose = 'artifact' OR attempt_id IS NOT NULL),
    CHECK ((purpose = 'staging') = (staging_path IS NOT NULL)),
    UNIQUE (id, attempt_id, purpose)
) STRICT;
CREATE UNIQUE INDEX one_reservation_lease_per_attempt ON leases (attempt_id) WHERE purpose = 'reservation' AND released_at_ns IS NULL;
CREATE INDEX leases_by_expiry ON leases (expires_at_ns) WHERE released_at_ns IS NULL;

CREATE TABLE finalization_claims (
    slot_id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT NOT NULL UNIQUE,
    lease_id TEXT NOT NULL UNIQUE,
    lease_purpose TEXT NOT NULL DEFAULT 'finalization' CHECK (lease_purpose = 'finalization'),
    FOREIGN KEY (attempt_id, slot_id) REFERENCES attempts (id, slot_id),
    FOREIGN KEY (lease_id, attempt_id, lease_purpose) REFERENCES leases (id, attempt_id, purpose)
) STRICT;

CREATE TABLE observations (
    id TEXT PRIMARY KEY NOT NULL,
    slot_id TEXT NOT NULL,
    producing_attempt_id TEXT NOT NULL,
    attempt_slot_id TEXT NOT NULL,
    valid INTEGER NOT NULL CHECK (valid IN (0, 1)),
    completion TEXT NOT NULL CHECK (completion IN ('not_required', 'reached', 'not_reached')),
    artifact_id TEXT NOT NULL,
    artifact_kind TEXT NOT NULL DEFAULT 'observation' CHECK (artifact_kind = 'observation'),
    record_json TEXT NOT NULL CHECK (json_valid(record_json) AND json_type(record_json) = 'object'),
    FOREIGN KEY (producing_attempt_id, attempt_slot_id) REFERENCES attempts (id, slot_id),
    FOREIGN KEY (attempt_slot_id, slot_id) REFERENCES attempt_observation_slots (attempt_slot_id, observation_slot_id),
    FOREIGN KEY (artifact_id, artifact_kind) REFERENCES artifacts (id, kind),
    UNIQUE (id, slot_id, valid)
) STRICT;
CREATE INDEX observations_by_attempt ON observations (producing_attempt_id);

CREATE TABLE selected_observations (
    slot_id TEXT PRIMARY KEY NOT NULL,
    observation_id TEXT NOT NULL UNIQUE,
    valid INTEGER NOT NULL DEFAULT 1 CHECK (valid = 1),
    selected_at_ns INTEGER NOT NULL CHECK (selected_at_ns >= 0),
    FOREIGN KEY (observation_id, slot_id, valid) REFERENCES observations (id, slot_id, valid)
) STRICT;

CREATE TABLE attempt_artifacts (
    attempt_id TEXT NOT NULL REFERENCES attempts (id),
    role TEXT NOT NULL CHECK (length(role) > 0),
    artifact_id TEXT NOT NULL REFERENCES artifacts (id),
    PRIMARY KEY (attempt_id, role, artifact_id)
) STRICT;
CREATE INDEX attempts_referencing_artifact ON attempt_artifacts (artifact_id);

CREATE TABLE observation_artifacts (
    observation_id TEXT NOT NULL REFERENCES observations (id),
    role TEXT NOT NULL CHECK (length(role) > 0),
    artifact_id TEXT NOT NULL REFERENCES artifacts (id),
    PRIMARY KEY (observation_id, role, artifact_id)
) STRICT;
CREATE INDEX observations_referencing_artifact ON observation_artifacts (artifact_id);

CREATE TABLE leased_artifacts (
    lease_id TEXT NOT NULL REFERENCES leases (id),
    artifact_id TEXT NOT NULL REFERENCES artifacts (id),
    PRIMARY KEY (lease_id, artifact_id)
) STRICT;
CREATE INDEX leases_referencing_artifact ON leased_artifacts (artifact_id);

CREATE TRIGGER attempts_start_reserved BEFORE INSERT ON attempts
WHEN NEW.state != 'reserved' OR NEW.revision != 0 OR NEW.updated_at_ns != NEW.started_at_ns
BEGIN SELECT RAISE(ABORT, 'attempts must start reserved'); END;

CREATE TRIGGER attempts_advance BEFORE UPDATE ON attempts
WHEN OLD.state IN ('accepted', 'censored', 'superseded', 'failed')
    OR NEW.id != OLD.id OR NEW.slot_id != OLD.slot_id OR NEW.retry_index != OLD.retry_index
    OR NEW.started_at_ns != OLD.started_at_ns OR NEW.revision != OLD.revision + 1
    OR NEW.updated_at_ns < OLD.updated_at_ns
BEGIN SELECT RAISE(ABORT, 'invalid attempt projection update'); END;

CREATE TRIGGER acceptance_requires_claim BEFORE UPDATE ON attempts
WHEN NEW.state = 'accepted' AND NOT EXISTS (
    SELECT 1 FROM finalization_claims WHERE slot_id = NEW.slot_id AND attempt_id = NEW.id
)
BEGIN SELECT RAISE(ABORT, 'acceptance requires the finalization claim'); END;

CREATE TRIGGER claims_require_finalizing_attempt BEFORE INSERT ON finalization_claims
WHEN NOT EXISTS (SELECT 1 FROM attempts WHERE id = NEW.attempt_id AND state = 'finalizing')
    OR EXISTS (SELECT 1 FROM attempts WHERE slot_id = NEW.slot_id AND state = 'accepted')
    OR NOT EXISTS (SELECT 1 FROM leases WHERE id = NEW.lease_id AND released_at_ns IS NULL)
BEGIN SELECT RAISE(ABORT, 'claim requires an unsatisfied slot and live finalizer'); END;

-- Expiration is evidence for recovery, not permission to steal the claim.
CREATE TRIGGER claims_release_after_terminal BEFORE DELETE ON finalization_claims
WHEN NOT EXISTS (SELECT 1 FROM attempts WHERE id = OLD.attempt_id AND state IN ('accepted', 'censored', 'superseded', 'failed'))
BEGIN SELECT RAISE(ABORT, 'resolve claimed attempt before releasing claim'); END;

CREATE TRIGGER claims_no_update BEFORE UPDATE ON finalization_claims
BEGIN SELECT RAISE(ABORT, 'finalization claims cannot change owners'); END;

CREATE TRIGGER leases_preserve_owner BEFORE UPDATE ON leases
WHEN OLD.released_at_ns IS NOT NULL OR NEW.id != OLD.id OR NEW.owner_token != OLD.owner_token
    OR NEW.attempt_id IS NOT OLD.attempt_id OR NEW.purpose != OLD.purpose
    OR NEW.staging_path IS NOT OLD.staging_path OR NEW.acquired_at_ns != OLD.acquired_at_ns
    OR NEW.expires_at_ns < OLD.expires_at_ns
BEGIN SELECT RAISE(ABORT, 'lease ownership is immutable'); END;

CREATE TRIGGER applicability_decisions_no_update BEFORE UPDATE ON applicability_decisions
BEGIN SELECT RAISE(ABORT, 'applicability_decisions is insert-only'); END;

CREATE TRIGGER applicability_decisions_no_delete BEFORE DELETE ON applicability_decisions
BEGIN SELECT RAISE(ABORT, 'applicability_decisions is insert-only'); END;

CREATE TRIGGER resolved_runs_no_update BEFORE UPDATE ON resolved_runs
BEGIN SELECT RAISE(ABORT, 'resolved_runs is insert-only'); END;

CREATE TRIGGER resolved_runs_no_delete BEFORE DELETE ON resolved_runs
BEGIN SELECT RAISE(ABORT, 'resolved_runs is insert-only'); END;

CREATE TRIGGER observation_slots_no_update BEFORE UPDATE ON observation_slots
BEGIN SELECT RAISE(ABORT, 'observation_slots is insert-only'); END;

CREATE TRIGGER observation_slots_no_delete BEFORE DELETE ON observation_slots
BEGIN SELECT RAISE(ABORT, 'observation_slots is insert-only'); END;

CREATE TRIGGER attempt_slots_no_update BEFORE UPDATE ON attempt_slots
BEGIN SELECT RAISE(ABORT, 'attempt_slots is insert-only'); END;

CREATE TRIGGER attempt_slots_no_delete BEFORE DELETE ON attempt_slots
BEGIN SELECT RAISE(ABORT, 'attempt_slots is insert-only'); END;

CREATE TRIGGER attempt_observation_slots_no_update BEFORE UPDATE ON attempt_observation_slots
BEGIN SELECT RAISE(ABORT, 'attempt_observation_slots is insert-only'); END;

CREATE TRIGGER attempt_observation_slots_no_delete BEFORE DELETE ON attempt_observation_slots
BEGIN SELECT RAISE(ABORT, 'attempt_observation_slots is insert-only'); END;

CREATE TRIGGER artifacts_no_update BEFORE UPDATE ON artifacts
BEGIN SELECT RAISE(ABORT, 'artifacts is insert-only'); END;

CREATE TRIGGER artifacts_no_delete BEFORE DELETE ON artifacts
BEGIN SELECT RAISE(ABORT, 'artifacts is insert-only'); END;

CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events is insert-only'); END;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'events is insert-only'); END;

CREATE TRIGGER attempt_transitions_no_update BEFORE UPDATE ON attempt_transitions
BEGIN SELECT RAISE(ABORT, 'attempt_transitions is insert-only'); END;

CREATE TRIGGER attempt_transitions_no_delete BEFORE DELETE ON attempt_transitions
BEGIN SELECT RAISE(ABORT, 'attempt_transitions is insert-only'); END;

CREATE TRIGGER observations_no_update BEFORE UPDATE ON observations
BEGIN SELECT RAISE(ABORT, 'observations is insert-only'); END;

CREATE TRIGGER observations_no_delete BEFORE DELETE ON observations
BEGIN SELECT RAISE(ABORT, 'observations is insert-only'); END;

CREATE TRIGGER selected_observations_no_update BEFORE UPDATE ON selected_observations
BEGIN SELECT RAISE(ABORT, 'selected_observations is insert-only'); END;

CREATE TRIGGER selected_observations_no_delete BEFORE DELETE ON selected_observations
BEGIN SELECT RAISE(ABORT, 'selected_observations is insert-only'); END;

CREATE TRIGGER attempt_artifacts_no_update BEFORE UPDATE ON attempt_artifacts
BEGIN SELECT RAISE(ABORT, 'attempt_artifacts is insert-only'); END;

CREATE TRIGGER attempt_artifacts_no_delete BEFORE DELETE ON attempt_artifacts
BEGIN SELECT RAISE(ABORT, 'attempt_artifacts is insert-only'); END;

CREATE TRIGGER observation_artifacts_no_update BEFORE UPDATE ON observation_artifacts
BEGIN SELECT RAISE(ABORT, 'observation_artifacts is insert-only'); END;

CREATE TRIGGER observation_artifacts_no_delete BEFORE DELETE ON observation_artifacts
BEGIN SELECT RAISE(ABORT, 'observation_artifacts is insert-only'); END;

CREATE TRIGGER leased_artifacts_no_update BEFORE UPDATE ON leased_artifacts
BEGIN SELECT RAISE(ABORT, 'leased_artifacts is insert-only'); END;

CREATE TRIGGER leased_artifacts_no_delete BEFORE DELETE ON leased_artifacts
BEGIN SELECT RAISE(ABORT, 'leased_artifacts is insert-only'); END;

CREATE TRIGGER attempts_no_delete BEFORE DELETE ON attempts
BEGIN SELECT RAISE(ABORT, 'attempts history cannot be deleted'); END;

CREATE TRIGGER leases_no_delete BEFORE DELETE ON leases
BEGIN SELECT RAISE(ABORT, 'leases history cannot be deleted'); END;
