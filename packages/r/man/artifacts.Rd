\name{result}
\alias{result}
\alias{dataset}
\alias{artifact}
\alias{evaluation}
\alias{worker_error}
\title{Describe Scientific Outputs}
\description{Construct callback values for binding API version 1. The CLI validates
scientific data, hashes completed files, and writes canonical artifact envelopes.}
\usage{
result(data = json_object(), files = list())
dataset(data = NULL, files = list())
artifact(path, media_type, representation = NULL)
evaluation(metrics, verdict)
worker_error(code, message, details = NULL)
}
\arguments{
  \item{data}{A named list of scientific data. NULL omits data.json for a dataset.}
  \item{files}{A list of artifact declarations for completed files.}
  \item{path}{A normalized file path relative to the assigned output directory.}
  \item{media_type}{The file's media type.}
  \item{representation}{Optional metadata describing shape, data type, byte
  order, or sparse layout.}
  \item{metrics}{A named list of independently computed metrics.}
  \item{verdict}{One of invalid, reached, not_reached, or not_required.}
  \item{code}{A structured failure code understood by the CLI.}
  \item{message}{A nonempty failure message.}
  \item{details}{Optional named list of diagnostic details.}
}
\details{Write files beneath context$output_dir and close them before returning.
The CLI rejects missing, duplicate, escaping, and undeclared files. Dataset and
result callbacks may return a plain named list when they produce only JSON data.
Use stop(worker_error(...)) to report a structured callback failure.

These constructors work without the CLI and perform no filesystem operations.}
\value{An R object describing the output or a structured error condition.}
\examples{
result(list(answer = 6))
dataset(list(values = list(1, 2, 3)))
evaluation(list(error = 0), "not_required")
artifact("coefficients.bin", "application/octet-stream")
}
