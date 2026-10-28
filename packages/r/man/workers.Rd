\name{serve_implementation}
\alias{serve_implementation}
\alias{serve_dataset_materializer}
\alias{serve_evaluator}
\alias{worker_error}
\alias{evaluation}
\title{Serve Version-1 Workers}
\description{Dispatch benchmark-owned callbacks over dedicated protocol pipes.
Metewand owns environments, source acquisition, seeds, scheduling, validation,
and publication.}
\usage{
serve_implementation(prepare, execute, result = NULL, reset = NULL, shutdown = NULL,
                     statistics = NULL, read_fd = NULL, write_fd = NULL,
                     result_data = NULL, fresh_sequence = FALSE)
serve_dataset_materializer(materialize, read_fd = NULL, write_fd = NULL)
serve_evaluator(evaluate, read_fd = NULL, write_fd = NULL, semantics = FALSE)
worker_error(code, message, details = NULL)
evaluation(metrics, verdict)
}
\arguments{
  \item{prepare}{Function of dataset directory, contract identity, problem
  parameters, implementation parameters, and seed; returns clean state.}
  \item{execute}{Function of state, budget, and observation control; returns
  updated state. Budget and observation control are NULL in one-shot mode.}
  \item{result}{Optional custom function of state and result directory; closes
  all result files and returns the relative manifest path. When omitted, the
  package writes the standard result envelope from execute output or result_data.}
  \item{result_data}{Optional function converting execute output to scientific
  JSON data outside diagnostic solve timing. Supply either this or result.}
  \item{fresh_sequence}{When TRUE, report fresh_sequence and pass the selected
  canonical observation-control object to execute. Its response echo is preserved
  independently of callback mutations. Budget remains NULL.}
  \item{reset}{Optional function of state that releases slot-scoped resources.
  Its return value is discarded. Without it, reset is unsupported.}
  \item{shutdown}{Optional function of state that releases resources before exit.}
  \item{statistics}{Optional function of state returning a diagnostic JSON object
  or NULL. This callback runs outside implementation timing.}
  \item{read_fd, write_fd}{Separate caller-owned pipe descriptors. By default,
  Unix workers read METEWAND_PROTOCOL_READ_FD and METEWAND_PROTOCOL_WRITE_FD;
  Windows workers use METEWAND_PROTOCOL_READ_HANDLE and
  METEWAND_PROTOCOL_WRITE_HANDLE. Streams are binary on both platforms.}
  \item{materialize}{Function of source directory, dataset parameters, seed, and
  output directory. Closes all files and returns the relative dataset manifest.}
  \item{evaluate}{Function of dataset directory, contract identity, problem
  parameters, result directory, and metrics path. Writes and closes metrics,
  then returns invalid, reached, not_reached, or not_required. Alternatively,
  return evaluation(metrics, verdict) to let the package write metrics.}
  \item{metrics}{A canonical JSON object of evaluator-owned metric data.}
  \item{verdict}{One of invalid, reached, not_reached, or not_required.}
  \item{semantics}{When TRUE, negotiate scientific rules and pass the complete
  contract semantics object after problem parameters in the evaluator callback.
  FALSE preserves the original five-argument callback.}
  \item{code}{One of invalid_request, unsupported_operation, operation_failed,
  or internal_error.}
  \item{message}{A nonempty string describing the failure.}
  \item{details}{An optional canonical JSON object.}
}
\details{Directory and contract arguments are strings. Parameters use the JSON
representations documented under \code{canonical_bytes}. Seeds are nonnegative
safe integers and never set random generators automatically.

Execution consumes a successful preparation even on failure. Failed prepare or
reset also invalidates prepared execution. Package implementation time covers only
the execute callback; result serialization completes before a response is sent.
The package duplicates descriptors and closes only its copies. Logs remain on stdout
and stderr. Frames use canonical JSON Lines with a 1 MiB limit including the
newline. Malformed frames and missing IDs raise metewand_protocol_error.

Use \code{stop(worker_error(...))} for a structured failure. Other callback errors
are logged to stderr and produce operation_failed. Oversized failures become
bounded internal_error responses.}
\value{Serve functions return invisibly after shutdown or a rejected handshake.
\code{worker_error} returns an error condition with class metewand_worker_error.
\code{evaluation} pairs metrics with an explicit scientific verdict.}
