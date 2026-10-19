# Worker environments

Every trusted local worker starts with a cleared environment, an owner-only
working directory, and umask `0077`. Files created with ordinary defaults have
mode `0600`, and directories have mode `0700`. The launcher sets the umask only
in the child, so concurrent launches do not change the orchestrator's settings.
Implementation, evaluator, materializer, and contract-owned dataset checker
processes share this launch path.

The runtime supplies `LANG=C`, `LC_ALL=C`, and `TZ=UTC`. It allocates the following
directories within each worker's private working directory:

| Variables | Relative directory |
| --- | --- |
| `HOME`, `R_USER` | `home` |
| `TMPDIR`, `TMP`, `TEMP` | `tmp` |
| `XDG_CONFIG_HOME` | `config` |
| `XDG_CACHE_HOME` | `cache` |
| `XDG_DATA_HOME` | `data` |
| `XDG_STATE_HOME` | `state` |
| `XDG_RUNTIME_DIR` | `runtime` |
| `PYTHONUSERBASE` | `data/python` |
| `PIP_CACHE_DIR` | `cache/pip` |
| `UV_CACHE_DIR` | `cache/uv` |
| `R_LIBS_USER` | `cache/r/library` |
| `JULIA_DEPOT_PATH` | `cache/julia` |

Each directory has mode `0700`, including intermediate directories. Workers also
receive `PYTHONNOUSERSITE=1` and `PYTHONDONTWRITEBYTECODE=1`. No ambient variable,
including `PATH`, user profiles, language search paths, or credentials, is copied
implicitly. A resolved backend can declare additional variables. `run` supplies
a minimal `PATH` containing the resolved program directory and `/usr/bin:/bin`.
Workers needing further software must select it through their resolved context.

`worker_environment::worker_environment_template` returns these fixed settings
with `{worker}/...` paths. `resolve_worker_environment` merges an explicit
allowlist with those settings and rejects conflicting fixed values, malformed
entries, and attempts to override protocol descriptor variables. Custom
`{worker}/...` directory values must use normalized relative paths. Validation
precedes directory allocation and process launch. Launch failure removes the
allocated directories, and consuming the process handle removes them after the
session finishes.

`run` records the templates in resolved launch identities. It substitutes
allocated paths only during launch. `PosixWorkerProcess::environment`,
`WorkerSession::environment`, and the terminal process output expose the actual
settings as a version-1 `worker-environment` provenance record. That record
contains the current directory, realized variables, and umask. Successful
attempts include it for both workers, and dataset construction receipts include
it for the materializer and optional checker. Ephemeral paths do not enter
resolved identities.

## Secret channels

A runtime caller declares a secret channel by placing `{secret}` as the entire
value of an allowlisted variable. For example, the resolved launch can contain
`TOKEN = "{secret}"`. The channel name and placeholder are public identity
inputs. The caller supplies the value separately through
`WorkerSecrets::new` and `LocalProcessExecutor::spawn_with_secrets`, or through
`launch_trusted_local_worker_with_secrets`. No ambient credential lookup occurs.
Values must be nonempty and NUL-free, and their channel names must match the
declarations exactly. Missing, extra, or conflicting bindings fail before launch.
The ordinary `spawn` path rejects unresolved secret declarations.

`WorkerSecrets` has no serialization or identity implementation, and its debug
representation shows channel names only. Realized provenance retains `{secret}`
instead of the value. The log drains replace literal secret bytes with
`[REDACTED]` before exposing their retained output, including overlapping values,
secrets split across reads, and secrets crossing the retained-byte limit. Logs
retain only the original bounded prefix after filtering; lookahead bytes never
fill space freed by a replacement. Total-byte counts describe the original
stream. Secret sessions also filter decoded response strings and object keys
before typed validation or provenance capture. Raw framing errors omit their input-dependent
diagnostics when a session carries secrets.

These channels support trusted workers. Workers must keep credentials out of
scientific result files and must not encode or transform them into other
outputs. Raw protocol accessors expose the wire stream to the caller; use
`WorkerSession` for filtered responses. The CLI does not yet accept secret
bindings. Private directories and filtered capture do not restrict the worker's
OS user from accessing other files or inspecting process environments.

## Compatibility and verification

Execution semantics, identity representation, wire protocol, and public-schema
compatibility remain at version 1. This implements the environment behavior
already specified for the unreleased version-1 contract. Resolved launches now
bind the explicit fixed values and templates, so their content identities change
without changing the identity transcript. The new provenance schema declares
version 1, and dataset construction receipts require the worker settings and a
nullable checker environment. Schema conformance vectors cover the new record
and reject invalid paths, names, umasks, and unknown fields.

Process tests inspect the actual environment, working directory, and file modes
after exec, including launches from a parent with hostile ambient settings and
umask `0777`. Other tests cover conflicting overrides, cleanup, identity
stability across secret values and storage locations, redacted diagnostics, and
bounded log capture with fragmented, overlapping, and non-UTF-8 output.
