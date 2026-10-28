\name{read_json}
\alias{read_json}
\alias{write_json}
\alias{json_object}
\alias{json_array}
\title{Read and Write Ordinary JSON}
\description{Small JSON utilities backed by jsonlite. The CLI owns canonical
serialization and artifact validation.}
\usage{
read_json(path)
write_json(path, value)
json_object(values = list())
json_array(values)
}
\arguments{
  \item{path}{A file path.}
  \item{value}{A JSON-compatible R value.}
  \item{values}{A named list for an object, or values to represent as an array.}
}
\details{Named lists represent objects and unnamed lists represent arrays.
Atomic vectors of length one become scalars. Use json_array to preserve an
array with one element, and json_object for an empty object. NULL represents
JSON null. Reading never simplifies arrays or objects into vectors or frames.
Numbers are written with 17 significant digits to preserve binary64 values.
Missing values, nonfinite numbers, unsafe integers, duplicate keys, unsupported
classes, and invalid Unicode are rejected. R cannot represent embedded NUL
strings; these are unsupported rather than silently truncated.}
\value{read_json returns the parsed value; write_json returns invisibly.
json_object returns a named list and json_array returns an AsIs value.}
\examples{
path <- tempfile()
write_json(path, list(answer = 6, values = json_array(1)))
read_json(path)
unlink(path)
}
