#' @rdname json
#' @export
json_object <- function(values = list()) {
  if (!is.list(values) || (length(values) && is.null(names(values)))) {
    stop("JSON objects must be named lists.", call. = FALSE)
  }
  if (!length(values)) names(values) <- character()
  values
}

#' @rdname json
#' @export
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
        !all(is.finite(value)) ||
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
    !all(attributes %in% c("names", "class")) ||
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

#' Read and Write Ordinary JSON
#'
#' Small JSON utilities backed by jsonlite. The CLI owns canonical serialization
#' and artifact validation.
#'
#' @param path A file path.
#' @param value A JSON-compatible R value.
#' @param values A named list for an object, or values to represent as an array.
#' @details
#' Named lists represent objects and unnamed lists represent arrays. Atomic
#' vectors of length one become scalars. Use `json_array()` to preserve an array
#' with one element, and `json_object()` for an empty object. `NULL` represents
#' JSON null. Reading never simplifies arrays or objects into vectors or frames.
#' Numbers are written with 17 significant digits to preserve binary64 values.
#' Missing values, nonfinite numbers, unsafe integers, duplicate keys,
#' unsupported classes, and invalid Unicode are rejected. R cannot represent
#' embedded NUL strings; these are unsupported rather than silently truncated.
#' @returns
#' `read_json()` returns the parsed value; `write_json()` returns invisibly.
#' `json_object()` returns a named list and `json_array()` returns an `AsIs`
#' value.
#' @examples
#' path <- tempfile()
#' write_json(path, list(answer = 6, values = json_array(1)))
#' read_json(path)
#' unlink(path)
#' @rdname json
#' @export
read_json <- function(path) {
  connection <- file(path, "rb")
  on.exit(close(connection))
  .parse_json(rawToChar(readBin(connection, "raw", n = file.info(path)$size)))
}

#' @rdname json
#' @export
write_json <- function(path, value) {
  bytes <- charToRaw(enc2utf8(.json_text(value)))
  connection <- file(path, "wb")
  on.exit(close(connection))
  writeBin(bytes, connection)
  invisible(NULL)
}

#' @rdname artifacts
#' @export
artifact <- function(path, media_type, representation = NULL) {
  structure(
    list(path = path, media_type = media_type, representation = representation),
    class = "metewand_artifact"
  )
}

#' @rdname artifacts
#' @export
dataset <- function(data = NULL, files = list()) {
  structure(
    list(data = if (is.null(data)) NULL else json_object(data), files = files),
    class = "metewand_dataset"
  )
}

#' Describe Scientific Outputs
#'
#' Construct callback values for binding API version 1. The CLI validates
#' scientific data, hashes completed files, and writes canonical artifact
#' envelopes.
#'
#' @param data A named list of scientific data. `NULL` omits `data.json` for a
#'   dataset.
#' @param files A list of artifact declarations for completed files.
#' @param path A normalized file path relative to the assigned output directory.
#' @param media_type The file's media type.
#' @param representation Optional metadata describing shape, data type, byte
#'   order, or sparse layout.
#' @param metrics A named list of independently computed metrics.
#' @param verdict One of `invalid`, `reached`, `not_reached`, or `not_required`.
#' @param code A structured failure code understood by the CLI.
#' @param message A nonempty failure message.
#' @param details Optional named list of diagnostic details.
#' @details
#' Write files beneath `context$output_dir` and close them before returning. The
#' CLI rejects missing, duplicate, escaping, and undeclared files. Dataset and
#' result callbacks may return a plain named list when they produce only JSON
#' data. Use `stop(worker_error(...))` to report a structured callback failure.
#'
#' These constructors work without the CLI and perform no filesystem operations.
#' @returns An R object describing the output or a structured error condition.
#' @examples
#' result(list(answer = 6))
#' dataset(list(values = list(1, 2, 3)))
#' evaluation(list(error = 0), "not_required")
#' artifact("coefficients.bin", "application/octet-stream")
#' @rdname artifacts
#' @export
result <- function(data = json_object(), files = list()) {
  structure(
    list(data = json_object(data), files = files),
    class = "metewand_result"
  )
}

#' @rdname artifacts
#' @export
evaluation <- function(metrics, verdict) {
  structure(
    list(metrics = json_object(metrics), verdict = verdict),
    class = "metewand_evaluation"
  )
}

#' @rdname artifacts
#' @export
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

#' Register Stable Scientific Callbacks
#'
#' Delegate binding API version 1 callbacks to a CLI-supplied adapter. The
#' package contains no worker protocol implementation.
#'
#' @param ... Named implementation callbacks described below.
#' @param materialize Function of context returning a dataset or named list.
#' @param evaluate Function of context returning an evaluation or, without
#'   declared scientific rules, a named list of metrics.
#' @details
#' Implementations require `prepare(context)`, which returns state, and
#' `execute(state, context)`, which returns solver output. Optional
#' `result(output, context)` converts that output into a result or named list;
#' `statistics(output, context)` returns diagnostics. Optional
#' `reset(state, context)` clears slot state, and `shutdown(state, context)`
#' releases resources. Execution consumes preparation, even on failure. The
#' original preparation state remains available for cleanup.
#'
#' Optional `check_applicability(context)` returns a support decision before
#' preparation. Unsupported optional callbacks fail in the CLI adapter, allowing
#' future callbacks without a package release.
#'
#' Context supplies `parameters`, `problem_parameters`, `seed`, `semantics`,
#' assigned input and output directories, and `observation_control` where
#' applicable. Additional optional fields may appear without changing callback
#' signatures. Neither seeds nor random generators are set automatically.
#'
#' Select `protocol-transport: socket` for lifecycle workers. Ordinary function
#' launches select managed transport automatically. Standard output and standard
#' error remain log channels. The CLI owns timing, protocol negotiation,
#' artifact finalization, and publication. R diagnostic solve time is
#' unavailable.
#'
#' Serving requires a Metewand CLI that supports binding API version 1; package
#' and CLI release numbers need not match.
#' @returns Returns invisibly after shutdown.
#' @rdname workers
#' @export
serve_implementation <- function(...) .serve("implementation", list(...))

#' @rdname workers
#' @export
serve_dataset_materializer <- function(materialize) {
  .serve("dataset_materializer", list(materialize = materialize))
}

#' @rdname workers
#' @export
serve_evaluator <- function(evaluate) {
  .serve("problem_evaluator", list(evaluate = evaluate))
}

#' Serve an Ordinary Benchmark Function
#'
#' Load a manifest-selected function inside a worker process and adapt it to the
#' CLI-owned dataset, implementation, or evaluator lifecycle.
#'
#' @param role One of `dataset_materializer`, `implementation`, or
#'   `problem_evaluator`. When `NULL`, read role, file, and name from the three
#'   trailing command arguments.
#' @param file The selected source file, sourced during the worker handshake.
#' @param name The name of the function defined in that file.
#' @details
#' Dataset functions take `context`; implementation functions take `data` and
#' `context`; evaluator functions take `data`, `result`, and `context`. Context
#' is a list containing `parameters`, `problem_parameters`, `seed`, and
#' `semantics`. Seeds are available for generation and solving; scientific rules
#' are available for evaluation. Unavailable objects are empty JSON objects and
#' unavailable seeds are `NULL`. Callbacks return named lists or explicit output
#' values. The CLI writes `data.json`, dataset inventories, and standard result
#' and metric envelopes.
#'
#' Without validity or completion rules, plain metric objects receive the
#' verdict `not_required`. Declared rules require an explicit evaluation object.
#' Completion verdicts require completion rules. Use lifecycle workers for
#' profiles, custom artifacts, and separate preparation.
#' @returns Returns invisibly after shutdown or a rejected handshake.
#' @rdname functions
#' @export
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
