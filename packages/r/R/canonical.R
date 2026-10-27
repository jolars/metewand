.canonical_error <- function(message) {
  stop(structure(list(message = message, call = NULL),
                 class = c("metewand_canonical_error", "error", "condition")))
}

.native <- function(symbol, ..., error = .canonical_error) {
  result <- .Call(symbol, ...)
  if (is.character(result)) error(result)
  result[[1L]]
}

# R cannot store embedded NULs in character strings. This explicit wrapper keeps
# those valid JSON strings lossless, including when they occur as object keys.
json_string <- function(bytes) {
  if (!is.raw(bytes) || !is.null(attributes(bytes))) {
    .canonical_error("JSON string bytes must be an unclassed raw vector.")
  }
  value <- structure(bytes, class = "metewand_string")
  canonical_bytes(value)
  value
}

json_object <- function(values = list(), keys = names(values)) {
  if (!is.list(values) || is.object(values)) .canonical_error("Object values must be a list.")
  if (is.null(keys)) {
    if (length(values)) .canonical_error("Object keys must be supplied.")
    keys <- character()
  }
  if (is.character(keys)) keys <- as.list(keys)
  if (!is.list(keys) || length(keys) != length(values)) {
    .canonical_error("Supply one string key for every object value.")
  }
  if (all(vapply(keys, function(key) is.character(key) && length(key) == 1L, logical(1)))) {
    value <- setNames(unname(values), unlist(keys, use.names = FALSE))
    if (!length(keys)) names(value) <- character()
  } else {
    value <- structure(unname(values), class = "metewand_object", keys = keys)
    names(value) <- vapply(keys, function(key) if (is.character(key)) key else "", character(1))
  }
  canonical_bytes(value)
  value
}

canonical_bytes <- function(value) .native(C_mw_encode, value)

parse_json <- function(source) {
  if (is.character(source) && length(source) == 1L && !is.na(source) && !is.object(source)) {
    source <- charToRaw(enc2utf8(source))
  }
  if (!is.raw(source) || !is.null(attributes(source))) {
    .canonical_error("JSON input must be UTF-8 bytes or a character scalar.")
  }
  .native(C_mw_parse, source)
}
