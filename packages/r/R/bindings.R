# These values describe scientific outputs; the CLI owns their artifact format.
json_object <- function(values = list()) {
  if (!is.list(values) || (length(values) && is.null(names(values)))) {
    stop("JSON objects must be named lists.", call. = FALSE)
  }
  if (!length(values)) names(values) <- character()
  values
}

json_array <- function(values) I(unname(values))

.validate_json <- function(value) {
  if (is.null(value)) {
    return(invisible(NULL))
  }
  if (is.object(value) && !identical(class(value), "AsIs")) {
    stop(
      "Convert classed values explicitly before writing JSON.",
      call. = FALSE
    )
  }
  if (is.list(value)) {
    keys <- names(value)
    if (!is.null(keys) && (anyNA(keys) || anyDuplicated(keys))) {
      stop("JSON object keys must be unique and nonmissing.", call. = FALSE)
    }
    if (!is.null(keys)) {
      .validate_json(keys)
    }
    lapply(value, .validate_json)
  } else if (is.character(value)) {
    if (
      anyNA(value) ||
        any(Encoding(value) == "bytes") ||
        anyNA(iconv(enc2utf8(value), from = "UTF-8", to = "UTF-8", sub = NA))
    ) {
      stop(
        "JSON strings must contain valid UTF-8 and cannot be missing.",
        call. = FALSE
      )
    }
  } else if (is.logical(value) || is.numeric(value)) {
    if (
      anyNA(value) ||
        any(!is.finite(value)) ||
        (is.numeric(value) &&
          any(value == trunc(value) & abs(value) > 9007199254740991))
    ) {
      stop(
        "JSON numbers must be finite; integers must be safe binary64 values. Missing values are unsupported.",
        call. = FALSE
      )
    }
  } else {
    stop("Unsupported JSON value. Convert it explicitly.", call. = FALSE)
  }
  attributes <- names(attributes(value))
  if (
    any(!attributes %in% c("names", "class")) ||
      (!is.list(value) && !is.null(names(value)))
  ) {
    stop(
      "Convert attributed values explicitly before writing JSON.",
      call. = FALSE
    )
  }
  invisible(NULL)
}

.json_text <- function(value) {
  .validate_json(value)
  scalar <- function(value) {
    as.character(jsonlite::toJSON(
      value,
      auto_unbox = TRUE,
      digits = I(17),
      null = "null",
      na = "null",
      pretty = FALSE
    ))
  }
  encode <- function(value) {
    if (!is.list(value)) {
      return(scalar(value))
    }
    members <- vapply(value, encode, character(1))
    if (is.null(names(value))) {
      return(paste0("[", paste(members, collapse = ","), "]"))
    }
    if (!length(value)) {
      return("{}")
    }
    # jsonlite renames empty list keys; composing containers preserves their names.
    keys <- vapply(names(value), scalar, character(1), USE.NAMES = FALSE)
    paste0("{", paste(paste0(keys, ":", members), collapse = ","), "}")
  }
  encode(value)
}

.parse_json <- function(source) {
  # R cannot store NULs, and jsonlite replaces lone high surrogates with "?".
  # Consume escaped backslashes and valid surrogate pairs together.
  escapes <- regmatches(
    source,
    gregexpr(
      r"(\\(?:u[dD][89aAbB][0-9a-fA-F]{2}\\u[dD][c-fC-F][0-9a-fA-F]{2}|u[0-9a-fA-F]{4}|.))",
      source,
      perl = TRUE
    )
  )[[1L]]
  if (any(escapes == "\\u0000")) {
    stop("Embedded NUL strings are unsupported in R.", call. = FALSE)
  }
  if (any(grepl(r"(^\\u[dD][89a-fA-F][0-9a-fA-F]{2}$)", escapes))) {
    stop("JSON strings cannot contain lone surrogates.", call. = FALSE)
  }
  # jsonlite's parser permits comments and BOMs, so validate the source separately.
  if (!jsonlite::validate(source)) {
    stop("Invalid JSON source.", call. = FALSE)
  }
  value <- jsonlite::parse_json(source, simplifyVector = FALSE)
  .validate_json(value)
  value
}

read_json <- function(path) {
  connection <- file(path, "rb")
  on.exit(close(connection))
  .parse_json(rawToChar(readBin(connection, "raw", n = file.info(path)$size)))
}

write_json <- function(path, value) {
  bytes <- charToRaw(enc2utf8(.json_text(value)))
  connection <- file(path, "wb")
  on.exit(close(connection))
  writeBin(bytes, connection)
  invisible(NULL)
}

artifact <- function(path, media_type, representation = NULL) {
  structure(
    list(path = path, media_type = media_type, representation = representation),
    class = "metewand_artifact"
  )
}

dataset <- function(data = NULL, files = list()) {
  structure(
    list(data = if (is.null(data)) NULL else json_object(data), files = files),
    class = "metewand_dataset"
  )
}

result <- function(data = json_object(), files = list()) {
  structure(
    list(data = json_object(data), files = files),
    class = "metewand_result"
  )
}

evaluation <- function(metrics, verdict) {
  structure(
    list(metrics = json_object(metrics), verdict = verdict),
    class = "metewand_evaluation"
  )
}

worker_error <- function(code, message, details = NULL) {
  structure(
    list(message = message, call = NULL, code = code, details = details),
    class = c("metewand_worker_error", "error", "condition")
  )
}

.serve <- function(role, callbacks) {
  path <- Sys.getenv("METEWAND_ADAPTER_PATH", unset = "")
  if (!nzchar(path)) {
    stop(
      "Run this worker through the Metewand CLI to supply its adapter.",
      call. = FALSE
    )
  }
  adapter <- new.env(parent = baseenv())
  sys.source(path, envir = adapter)
  if (!identical(adapter$binding_api_version, 1L)) {
    stop(
      "The Metewand CLI does not support binding API version 1; update the CLI.",
      call. = FALSE
    )
  }
  adapter$serve_bindings(
    1L,
    role,
    callbacks,
    list(
      name = "metewand-r",
      version = as.character(utils::packageVersion("metewand"))
    ),
    list(encode = .json_text, decode = .parse_json)
  )
}

serve_implementation <- function(...) .serve("implementation", list(...))
serve_dataset_materializer <- function(materialize) {
  .serve("dataset_materializer", list(materialize = materialize))
}
serve_evaluator <- function(evaluate) {
  .serve("problem_evaluator", list(evaluate = evaluate))
}

serve_function <- function(role = NULL, file = NULL, name = NULL) {
  if (is.null(role)) {
    arguments <- commandArgs(trailingOnly = TRUE)
    if (length(arguments) != 3L) {
      stop(
        "Expected worker role, source file, and function name.",
        call. = FALSE
      )
    }
    role <- arguments[[1L]]
    file <- arguments[[2L]]
    name <- arguments[[3L]]
  }
  .serve(role, list(`function` = list(file, name)))
}
