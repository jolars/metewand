\name{serve_implementation}
\alias{serve_implementation}
\alias{serve_dataset_materializer}
\alias{serve_evaluator}
\alias{worker_error}
\title{Serve Version-1 One-Shot Workers}
\description{Dispatch benchmark-owned callbacks over dedicated protocol pipes.
Metewand owns environments, source acquisition, seeds, scheduling, validation,
and publication.}
\usage{
serve_implementation(prepare, execute, result, reset = NULL, shutdown = NULL,
                     statistics = NULL, read_fd = NULL, write_fd = NULL)
serve_dataset_materializer(materialize, read_fd = NULL, write_fd = NULL)
serve_evaluator(evaluate, read_fd = NULL, write_fd = NULL)
worker_error(code, message, details = NULL)
}
\arguments{
  \item{prepare}{Function of dataset directory, contract identity, problem
  parameters, implementation parameters, and seed; returns clean state.}
  \item{execute}{Function of state, budget, and observation control; returns
  updated state. Budget and observation control are NULL in one-shot mode.}
  \item{result}{Function of state and result directory; closes all result files
  and returns the relative manifest path.}
  \item{reset}{Optional function of state that releases slot-scoped resources.
  Its return value is discarded. Without it, reset is unsupported.}
  \item{shutdown}{Optional function of state that releases resources before exit.}
  \item{statistics}{Optional function of state returning a diagnostic JSON object
  or NULL. This callback runs outside implementation timing.}
  \item{read_fd, write_fd}{Separate inherited POSIX pipe descriptors. Defaults
  read METEWAND_PROTOCOL_READ_FD and METEWAND_PROTOCOL_WRITE_FD.}
  \item{materialize}{Function of source directory, dataset parameters, seed, and
  output directory. Closes all files and returns the relative dataset manifest.}
  \item{evaluate}{Function of dataset directory, contract identity, problem
  parameters, result directory, and metrics path. Writes and closes metrics,
  then returns invalid, reached, not_reached, or not_required.}
  \item{code}{One of invalid_request, unsupported_operation, operation_failed,
  or internal_error.}
  \item{message}{A nonempty string describing the failure.}
  \item{details}{An optional canonical JSON object.}
}
\details{Directory and contract arguments are strings. Parameters use the JSON
representations documented under \code{canonical_bytes}. Seeds are nonnegative
safe integers and never set random generators automatically.

Execution consumes a successful preparation even on failure. Failed prepare or
reset also invalidates prepared execution. SDK implementation time covers only
the execute callback; result serialization completes before a response is sent.
The SDK duplicates descriptors and closes only its copies. Logs remain on stdout
and stderr. Frames use canonical JSON Lines with a 1 MiB limit including the
newline. Malformed frames and missing IDs raise metewand_protocol_error.

Use \code{stop(worker_error(...))} for a structured failure. Other callback errors
are logged to stderr and produce operation_failed. Oversized failures become
bounded internal_error responses.}
\value{Serve functions return invisibly after shutdown or a rejected handshake.
\code{worker_error} returns an error condition with class metewand_worker_error.}
