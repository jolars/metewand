.protocol_error <- function(message) {
  stop(structure(list(message = message, call = NULL),
                 class = c("metewand_protocol_error", "error", "condition")))
}

worker_error <- function(code, message, details = NULL) {
  if (!is.character(code) || length(code) != 1L || is.na(code) ||
      !code %in% c("invalid_request", "unsupported_operation", "operation_failed", "internal_error")) {
    stop("Unknown version-1 worker failure code.", call. = FALSE)
  }
  if (!.nonempty_string(message)) stop("Worker failure messages must be nonempty strings.", call. = FALSE)
  canonical_bytes(message)
  if (!is.null(details)) .object(details, "Worker failure details")
  # Conditions need an R character message, but the wire value remains lossless.
  structure(list(message = if (is.character(message)) message else "Worker failure.",
                 wire_message = message, call = NULL, code = code, details = details),
            class = c("metewand_worker_error", "error", "condition"))
}
.fail <- function(code, message) stop(worker_error(code, message))

.fields <- list(
  hello = c(protocols = "protocols", role = "role", worker_id = "worker_id"),
  materialize = c(source_dir = "string", dataset_parameters = "object", dataset_seed = "seed", output_dir = "string"),
  prepare = c(dataset_dir = "string", problem_contract_id = "string", problem_parameters = "object",
              implementation_parameters = "object", implementation_seed = "seed"),
  execute = c(result_dir = "string"), reset = character(),
  evaluate = c(dataset_dir = "string", problem_contract_id = "string", problem_parameters = "object",
               result_dir = "string", metrics_path = "string"), shutdown = character())
.methods <- list(implementation = c("prepare", "execute", "reset", "shutdown"),
                 dataset_materializer = c("materialize", "shutdown"),
                 problem_evaluator = c("evaluate", "shutdown"))

.envelope <- function(request) {
  if (!.is_object(request) || !.nonempty_string(request[["id"]])) {
    .protocol_error("A request must be an object with a nonempty string ID.")
  }
}
.validate_request <- function(request) {
  .envelope(request)
  if (is.object(request)) .fail("invalid_request", "Unexpected request fields.")
  method <- request[["method"]]
  if (!.nonempty_string(method)) .fail("invalid_request", "A request must name a method.")
  if (!is.character(method) || !method %in% names(.fields)) .fail("unsupported_operation", "Unsupported method.")
  fields <- .fields[[method]]
  if (!setequal(names(request), c("id", "method", names(fields)))) {
    .fail("invalid_request", paste0("Unexpected or missing fields for ", method, "."))
  }
  for (key in names(fields)) {
    value <- request[[key]]
    valid <- switch(fields[[key]],
      string = .nonempty_string(value), object = .is_object(value), seed = .is_integer(value),
      role = is.character(value) && length(value) == 1L && value %in% names(.methods),
      worker_id = is.character(value) && length(value) == 1L &&
        grepl("^mw1-resolved-worker-[0-9a-f]{64}$", value),
      protocols = is.list(value) && is.null(attributes(value)) && length(value) > 0L &&
        all(vapply(value, .is_integer, logical(1), maximum = 2^32 - 1)) && !anyDuplicated(unlist(value)))
    if (!valid) .fail("invalid_request", paste0("Invalid ", key, " for ", method, "."))
  }
  method
}

.encode_response <- function(value) {
  frame <- c(canonical_bytes(value), as.raw(10))
  if (length(frame) > 1024 * 1024) stop("Response exceeds the 1 MiB protocol line limit.", call. = FALSE)
  frame
}
.encode_failure <- function(id, error) {
  failure <- function(error) {
    payload <- list(code = error$code, message = error$wire_message)
    if (!is.null(error$details)) payload$details <- error$details
    .encode_response(list(id = id, ok = FALSE, error = payload))
  }
  tryCatch(failure(error), error = function(e) {
    tryCatch(failure(worker_error("internal_error", "Worker failure could not be encoded within the protocol limit.")),
      error = function(e) .protocol_error("Request ID leaves no room for a bounded failure response."))
  })
}
.completed_manifest <- function(directory, manifest) {
  path <- .output_path(directory, manifest)
  if (!file_test("-f", path)) stop("The callback returned before writing its manifest.", call. = FALSE)
  manifest
}
.clock <- function() .native(C_mw_clock, error = stop)

.serve <- function(worker, role, reader, writer) {
  ready <- FALSE
  prepared <- FALSE
  state <- NULL
  dispatch <- function(request) {
    method <- .validate_request(request)
    if (!ready) {
      if (method != "hello" || request$role != role) {
        .fail("invalid_request", "The first request must negotiate this worker's role.")
      }
      if (!1 %in% unlist(request$protocols)) .fail("unsupported_operation", "No offered protocol version is supported.")
      ready <<- TRUE
      return(list(protocol = 1, worker_id = request$worker_id,
                  sdk = list(name = "metewand-r", version = "0.1.0"),
                  capabilities = if (role == "implementation") list("one_shot") else list()))
    }
    if (method == "hello") .fail("invalid_request", "The worker session has already negotiated a protocol.")
    if (!method %in% .methods[[role]]) .fail("unsupported_operation", "This role does not accept the requested method.")
    if (method == "prepare") {
      prepared <<- FALSE
      state <<- NULL
      state <<- worker$prepare(request$dataset_dir, request$problem_contract_id,
                              request$problem_parameters, request$implementation_parameters,
                              request$implementation_seed)
      prepared <<- TRUE
    } else if (method == "execute") {
      if (!prepared) .fail("invalid_request", "Execute requires a successful prepare.")
      prepared <<- FALSE
      execute <- worker$execute
      start <- .clock()
      executed <- execute(state, NULL, NULL)
      elapsed <- .clock() - start
      state <<- executed
      statistics <- if (is.function(worker$statistics)) worker$statistics(state) else NULL
      if (is.null(statistics)) statistics <- json_object()
      statistics <- .object(statistics, "Implementation statistics")
      manifest <- .completed_manifest(request$result_dir, worker$result(state, request$result_dir))
      return(list(result = list(manifest = manifest), statistics = statistics,
                  implementation_time_ns = if (.is_integer(elapsed)) elapsed else NULL))
    } else if (method == "reset") {
      prepared <<- FALSE
      previous <- state
      state <<- NULL
      if (!is.function(worker$reset)) .fail("unsupported_operation", "This implementation does not support reset.")
      worker$reset(previous)
    } else if (method == "materialize") {
      manifest <- worker(request$source_dir, request$dataset_parameters, request$dataset_seed, request$output_dir)
      return(list(dataset = list(manifest = .completed_manifest(request$output_dir, manifest))))
    } else if (method == "evaluate") {
      verdict <- worker(request$dataset_dir, request$problem_contract_id, request$problem_parameters,
                        request$result_dir, request$metrics_path)
      if (!is.character(verdict) || length(verdict) != 1L || is.na(verdict) ||
          !verdict %in% c("invalid", "reached", "not_reached", "not_required")) {
        stop("Evaluators must return a version-1 scientific verdict.", call. = FALSE)
      }
      if (!file_test("-f", request$metrics_path)) stop("The evaluator returned before writing metrics.", call. = FALSE)
      return(list(verdict = verdict))
    } else if (method == "shutdown" && role == "implementation" && is.function(worker$shutdown)) {
      worker$shutdown(state)
    }
    list()
  }
  repeat {
    bytes <- .native(C_mw_read, reader, error = .protocol_error)
    request <- tryCatch(parse_json(bytes), error = function(e) .protocol_error(conditionMessage(e)))
    .envelope(request)
    success <- FALSE
    frame <- tryCatch({
      payload <- dispatch(request)
      frame <- .encode_response(c(list(id = request$id, ok = TRUE), payload))
      success <- TRUE
      frame
    }, metewand_worker_error = function(e) .encode_failure(request$id, e),
    error = function(e) {
      cat("Worker callback or output serialization failed: ", conditionMessage(e), "\n", sep = "", file = stderr())
      .encode_failure(request$id, worker_error("operation_failed",
        "Worker callback or output serialization failed; see the worker log."))
    })
    if (typeof(writer) == "externalptr") .native(C_mw_write, writer, frame, error = .protocol_error)
    else { writeBin(frame, writer); flush(writer) }
    if (!ready || (identical(request$method, "shutdown") && success)) return(invisible(NULL))
  }
}

.serve_fds <- function(worker, role, read_fd, write_fd) {
  if (is.null(read_fd)) read_fd <- suppressWarnings(as.double(Sys.getenv("METEWAND_PROTOCOL_READ_FD")))
  if (is.null(write_fd)) write_fd <- suppressWarnings(as.double(Sys.getenv("METEWAND_PROTOCOL_WRITE_FD")))
  if (!.is_integer(read_fd, .Machine$integer.max) || !.is_integer(write_fd, .Machine$integer.max)) {
    stop("Separate protocol read and write descriptors must be supplied.", call. = FALSE)
  }
  pipes <- .native(C_mw_open, read_fd, write_fd, error = stop)
  on.exit(.Call(C_mw_close, pipes))
  .serve(worker, role, pipes, pipes)
}

serve_implementation <- function(prepare, execute, result, reset = NULL, shutdown = NULL,
                                 statistics = NULL, read_fd = NULL, write_fd = NULL) {
  callbacks <- list(prepare = prepare, execute = execute, result = result,
                    reset = reset, shutdown = shutdown, statistics = statistics)
  if (!all(vapply(callbacks[1:3], is.function, logical(1))) ||
      !all(vapply(callbacks[4:6], function(f) is.null(f) || is.function(f), logical(1)))) {
    stop("Supply prepare, execute, and result functions and optional reset, shutdown, and statistics functions.", call. = FALSE)
  }
  .serve_fds(callbacks, "implementation", read_fd, write_fd)
}

serve_dataset_materializer <- function(materialize, read_fd = NULL, write_fd = NULL) {
  if (!is.function(materialize)) stop("Expected a materializer callback.", call. = FALSE)
  .serve_fds(materialize, "dataset_materializer", read_fd, write_fd)
}

serve_evaluator <- function(evaluate, read_fd = NULL, write_fd = NULL) {
  if (!is.function(evaluate)) stop("Expected an evaluator callback.", call. = FALSE)
  .serve_fds(evaluate, "problem_evaluator", read_fd, write_fd)
}
