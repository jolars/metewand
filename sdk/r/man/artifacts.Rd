\name{write_result}
\alias{read_json}
\alias{write_json}
\alias{file_entry}
\alias{write_dataset_manifest}
\alias{write_result}
\alias{write_metrics}
\title{Canonical Documents and Worker Output Envelopes}
\description{Read and write canonical documents, hash completed files, and write
the existing version-1 dataset, result, and metrics envelopes.}
\usage{
read_json(path)
write_json(path, value)
file_entry(directory, path, media_type, representation = NULL)
write_dataset_manifest(directory, files, manifest = "dataset-manifest.json")
write_result(directory, data, schema = NULL, files = list(), manifest = "result.json")
write_metrics(path, data, schema = NULL)
}
\arguments{
  \item{path}{A document path, or a normalized relative inventory path.}
  \item{value}{A value accepted by \code{canonical_bytes}.}
  \item{directory}{The assigned output directory.}
  \item{media_type}{An explicit nonempty media type string.}
  \item{representation}{Optional object containing \code{shape} as a list of
  nonnegative safe integers, \code{data_type}, \code{byte_order}, or
  \code{sparse_layout}. Byte order is \code{little}, \code{big}, or
  \code{not_applicable}.}
  \item{files}{An unnamed list of explicit file inventory entries.}
  \item{manifest}{A normalized manifest path relative to the output directory.}
  \item{data}{A problem-owned JSON object.}
  \item{schema}{An optional repository-relative problem schema path. Omit it to
  use the selected problem contract's schema, including an inline schema.}
}
\details{Files are closed before write helpers return. Inventories are sorted by
UTF-8 path bytes and cannot repeat paths or include their own manifest. Paths
must stay under the assigned directory, including through symlinks. Parent
directories are not created automatically. Authors supply representation metadata
and scientific semantics. Metewand independently verifies schemas, complete
trees, hashes, sizes, and problem contracts before publication.}
\value{\code{read_json} returns the parsed value. \code{file_entry} returns an
inventory object containing path, media type, SHA-256, size, and any supplied
representation. \code{write_dataset_manifest} and \code{write_result} return
relative manifest paths. Other write helpers invisibly return NULL.}
