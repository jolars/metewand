\name{canonical_bytes}
\alias{canonical_bytes}
\alias{parse_json}
\alias{json_object}
\alias{json_string}
\title{Restricted Canonical JSON}
\description{Parse and encode the version-1 Metewand JSON domain without coercing
library objects, simplifying arrays, or losing object keys.}
\usage{
canonical_bytes(value)
parse_json(source)
json_object(values = list(), keys = names(values))
json_string(bytes)
}
\arguments{
  \item{value}{A JSON value represented by unclassed scalars, lists, or explicit
  Metewand string and object wrappers.}
  \item{source}{UTF-8 raw bytes or a character scalar containing JSON.}
  \item{values}{A list of object values.}
  \item{keys}{A character vector or list of string keys, one per value.}
  \item{bytes}{Unclassed raw UTF-8 bytes, possibly containing NULs.}
}
\details{NULL represents JSON null, unnamed lists represent arrays, and named
lists represent objects. Use \code{json_object()} for an empty object. Atomic
vectors must have length one and no attributes. Missing values, nonfinite
numbers, unsafe integral numbers, duplicate keys, invalid UTF-8, and lone
surrogates are rejected. Nesting is bounded to 128 levels. Canonical bytes follow
RFC 8785 with UTF-16 property ordering and negative zero normalized to zero.

R cannot store embedded NULs in character strings. Such strings use raw vectors
of class \code{metewand_string}. Objects with NUL-containing keys use class
\code{metewand_object} and retain the original key list in their \code{keys}
attribute. Values retain source order. Convert these wrappers explicitly before
passing them to scientific libraries.}
\value{\code{canonical_bytes} returns raw UTF-8 bytes without a newline.
\code{parse_json} returns the corresponding R value. \code{json_object} returns
an object list, and \code{json_string} returns a validated string wrapper.
Invalid values raise a \code{metewand_canonical_error}.}
\examples{
rawToChar(canonical_bytes(list(answer = 6, values = list(1, 2, 3))))
rawToChar(canonical_bytes(json_object()))
parse_json('{"answer":6}')
}
