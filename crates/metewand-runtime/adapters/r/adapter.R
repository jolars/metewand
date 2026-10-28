binding_api_version <- 1L

.object <- function(value) metewand::json_object(value)

.is_object <- function(value) is.list(value) && !is.null(names(value))
.nonempty_string <- function(value) is.character(value) && length(value) == 1L && !is.na(value) && nzchar(value)
.integer <- function(value, maximum) {
  is.numeric(value) && length(value) == 1L && is.finite(value) &&
    value == trunc(value) && value >= 0 && value <= maximum
}
.fail <- function(code, message) stop(metewand::worker_error(code, message))

.request_fields <- list(
  hello = c(protocols = "protocols", role = "role", worker_id = "worker_id"),
  materialize = c(source_dir = "string", dataset_parameters = "object", dataset_seed = "seed", output_dir = "string"),
  check_applicability = c(dataset_dir = "string", dataset_manifest = "string", problem_contract_id = "string",
                        problem_parameters = "object", implementation_parameters = "object"),
  prepare = c(dataset_dir = "string", problem_contract_id = "string", problem_parameters = "object",
              implementation_parameters = "object", implementation_seed = "seed"),
  execute = c(result_dir = "string"), reset = character(),
  evaluate = c(dataset_dir = "string", problem_contract_id = "string", problem_parameters = "object",
               problem_semantics = "object", result_dir = "string", metrics_path = "string"),
  shutdown = character())

.validate_request <- function(request) {
  method <- request[["method"]]
  if (!.nonempty_string(method)) .fail("invalid_request", "A request must name a method.")
  if (!method %in% names(.request_fields)) .fail("unsupported_operation", "Unsupported worker operation.")
  fields <- .request_fields[[method]]
  if (method == "execute" && "observation_control" %in% names(request)) {
    fields <- c(fields, observation_control = "object")
  }
  if (!setequal(names(request), c("id", "method", names(fields)))) {
    .fail("invalid_request", paste0("Unexpected or missing fields for ", method, "."))
  }
  for (key in names(fields)) {
    value <- request[[key]]
    valid <- switch(fields[[key]],
      string = .nonempty_string(value), object = .is_object(value),
      seed = .integer(value, 9007199254740991),
      role = .nonempty_string(value) && value %in% c("implementation", "dataset_materializer", "problem_evaluator"),
      worker_id = .nonempty_string(value) && grepl("^mw1-resolved-worker-[0-9a-f]{64}$", value),
      protocols = is.list(value) && is.null(names(value)) && length(value) > 0L &&
        all(vapply(value, .integer, logical(1), maximum = 2^32 - 1)) && !anyDuplicated(unlist(value)))
    if (!valid) .fail("invalid_request", paste0("Invalid ", key, " for ", method, "."))
  }
  method
}

.read_frame <- function(connection, decode) {
  buffer <- raw(1024L * 1024L)
  for (i in seq_along(buffer)) {
    byte <- readBin(connection, "raw", n = 1L)
    if (!length(byte)) stop("Protocol stream ended before a complete frame.")
    buffer[[i]] <- byte
    if (byte == as.raw(10L)) {
      text <- rawToChar(buffer[seq_len(i)])
      return(decode(text))
    }
  }
  stop("Protocol frame exceeds the 1 MiB limit.")
}

.context <- function(request) {
  parameters <- request$implementation_parameters
  if (is.null(parameters)) parameters <- request$dataset_parameters
  if (is.null(parameters)) parameters <- request$problem_parameters
  if (is.null(parameters)) parameters <- .object(list())
  context <- list(parameters = parameters,
                  problem_parameters = if (is.null(request$problem_parameters)) .object(list()) else request$problem_parameters,
                  seed = if (is.null(request$implementation_seed)) request$dataset_seed else request$implementation_seed,
                  semantics = if (is.null(request$problem_semantics)) .object(list()) else request$problem_semantics,
                  budget = NULL, observation_control = request$observation_control)
  for (name in names(request)) context[name] <- request[name]
  if (is.null(context$output_dir)) context$output_dir <- request$result_dir
  structure(context, class = "metewand_context")
}

.read_data <- function(directory) {
  path <- file.path(directory, "data.json")
  if (!file.exists(path) && !length(list.files(directory, all.files = TRUE, no.. = TRUE))) return(.object(list()))
  .object(metewand::read_json(path))
}

.draft <- function(directory, kind, data, files = list()) {
  files <- lapply(files, function(file) {
    if (!inherits(file, "metewand_artifact")) stop("Declare output files with artifact().")
    value <- unclass(file)
    if (is.null(value$representation)) value$representation <- NULL
    value
  })
  metewand::write_json(file.path(directory, ".metewand-output.json"),
                       list(schema_version = 1, kind = kind, data = data, files = files))
  ".metewand-output.json"
}

serve_bindings <- function(api_version, role, callbacks, metadata, codec) {
  if (!identical(api_version, binding_api_version)) stop("Unsupported binding API version; this CLI supports version 1.")
  if (!role %in% c("implementation", "dataset_materializer", "problem_evaluator")) stop("Unknown worker role.")
  callbacks <- Filter(function(callback) !is.null(callback), callbacks)
  initialize <- function() NULL
  selected <- NULL
  if (!is.null(callbacks[["function"]])) {
    reference <- callbacks[["function"]]
    initialize <- function() {
      environment <- new.env(parent = globalenv())
      sys.source(reference[[1L]], envir = environment, chdir = TRUE)
      selected <<- environment[[reference[[2L]]]]
      if (!is.function(selected)) stop("The selected source does not identify a callable function.")
    }
    callbacks <- switch(role,
      implementation = list(prepare = function(context) .read_data(context$dataset_dir),
                            execute = function(state, context) selected(state, context),
                            reset = function(state, context) NULL),
      dataset_materializer = list(materialize = function(context) selected(context)),
      problem_evaluator = list(evaluate = function(context) selected(.read_data(context$dataset_dir),
                               metewand::read_json(file.path(context$result_dir, "result.json"))$data, context)))
  }
  allowed <- switch(role,
    implementation = c("prepare", "execute", "result", "statistics", "reset", "shutdown", "check_applicability"),
    dataset_materializer = "materialize", problem_evaluator = "evaluate")
  if (is.null(names(callbacks)) || anyDuplicated(names(callbacks)) ||
      any(!names(callbacks) %in% allowed) || !all(vapply(callbacks, is.function, logical(1)))) {
    stop("Unsupported or noncallable worker callback.")
  }
  required <- switch(role, implementation = c("prepare", "execute"), dataset_materializer = "materialize", problem_evaluator = "evaluate")
  if (any(!required %in% names(callbacks))) stop("Missing required worker callback.")
  port <- as.integer(Sys.getenv("METEWAND_PROTOCOL_PORT"))
  connection <- socketConnection(host = "127.0.0.1", port = port, blocking = TRUE,
                                 open = "a+b", timeout = 31 * 24 * 3600, options = "no-delay")
  on.exit(close(connection))
  writeBin(charToRaw(paste0(Sys.getenv("METEWAND_PROTOCOL_TOKEN"), "\n")), connection)
  ready <- FALSE
  prepared <- FALSE
  state <- NULL
  context <- NULL
  dispatch <- function(request) {
    method <- .validate_request(request)
    if (!ready) {
      if (method != "hello" || !identical(request$role, role)) {
        .fail("invalid_request", "The first request must negotiate this worker's role.")
      }
      if (!1 %in% unlist(request$protocols)) .fail("unsupported_operation", "No offered protocol version is supported.")
      initialize()
      ready <<- TRUE
      capabilities <- list("managed_outputs")
      if (role == "implementation") {
        capabilities <- c(capabilities, list("one_shot", "fresh_sequence"))
        if (is.function(callbacks$check_applicability)) capabilities <- c(capabilities, list("applicability"))
      }
      if (role == "problem_evaluator") capabilities <- c(capabilities, list("problem_semantics"))
      return(list(protocol = 1, worker_id = request$worker_id, sdk = metadata, capabilities = capabilities))
    }
    if (method == "hello") .fail("invalid_request", "The worker session has already negotiated a protocol.")
    if (method == "prepare" && role == "implementation") {
      prepared <<- FALSE
      state <<- NULL
      context <<- .context(request)
      state <<- callbacks$prepare(context)
      prepared <<- TRUE
    } else if (method == "execute" && role == "implementation") {
      if (!prepared) .fail("invalid_request", "Execute requires a successful prepare.")
      prepared <<- FALSE
      context$output_dir <<- request$result_dir
      context["observation_control"] <<- list(request$observation_control)
      output <- callbacks$execute(state, context)
      statistics <- if (is.function(callbacks$statistics)) callbacks$statistics(output, context) else .object(list())
      if (is.null(statistics)) statistics <- .object(list())
      value <- if (is.function(callbacks$result)) callbacks$result(output, context) else output
      if (!inherits(value, "metewand_result")) value <- metewand::result(value)
      response <- list(draft = .draft(request$result_dir, "result", value$data, value$files),
                       statistics = .object(statistics), implementation_time_ns = NULL)
      if (!is.null(request$observation_control)) response$observation_control <- request$observation_control
      return(response)
    } else if (method == "check_applicability" && role == "implementation" && is.function(callbacks$check_applicability)) {
      return(callbacks$check_applicability(.context(request)))
    } else if (method == "reset" && role == "implementation") {
      prepared <<- FALSE
      if (!is.function(callbacks$reset)) stop(metewand::worker_error("unsupported_operation", "Reset is not implemented."))
      on.exit({ state <<- NULL })
      callbacks$reset(state, context)
    } else if (method == "materialize" && role == "dataset_materializer") {
      value <- callbacks$materialize(.context(request))
      if (!inherits(value, "metewand_dataset")) value <- metewand::dataset(value)
      return(list(draft = .draft(request$output_dir, "dataset", value$data, value$files)))
    } else if (method == "evaluate" && role == "problem_evaluator") {
      context <- .context(request)
      value <- callbacks$evaluate(context)
      completion <- length(context$semantics$one_shot_completion) > 0L
      validity <- length(context$semantics$validity) > 0L
      if (!inherits(value, "metewand_evaluation")) {
        if (validity || completion) stop("Scientific rules require an explicit evaluation verdict.")
        value <- metewand::evaluation(value, "not_required")
      }
      if (!value$verdict %in% c("invalid", "reached", "not_reached", "not_required")) stop("Invalid scientific verdict.")
      if ((!completion && value$verdict %in% c("reached", "not_reached")) ||
          (completion && value$verdict == "not_required")) stop("The evaluation verdict must match declared completion rules.")
      return(list(verdict = value$verdict, draft = .draft(dirname(request$metrics_path), "metrics", value$metrics)))
    } else if (method == "shutdown") {
      if (role == "implementation" && is.function(callbacks$shutdown)) callbacks$shutdown(state, context)
    } else {
      stop(metewand::worker_error("unsupported_operation", "Unsupported worker operation."))
    }
    .object(list())
  }
  repeat {
    request <- .read_frame(connection, codec$decode)
    if (!.is_object(request) || !.nonempty_string(request[["id"]])) stop("Missing protocol request ID.")
    success <- FALSE
    response <- tryCatch({
      payload <- dispatch(request)
      success <- TRUE
      c(list(id = request$id, ok = TRUE), payload)
    }, error = function(error) {
      payload <- tryCatch({
        if (!inherits(error, "metewand_worker_error")) stop("ordinary callback error")
        if (!is.character(error$code) || length(error$code) != 1L ||
            !error$code %in% c("invalid_request", "unsupported_operation", "operation_failed", "internal_error", "resource_limit")) {
          stop("Unsupported worker failure code.")
        }
        value <- list(code = error$code, message = conditionMessage(error))
        if (!is.character(value$message) || length(value$message) != 1L || !nzchar(value$message)) stop("Missing worker failure message.")
        if (!is.null(error$details)) value$details <- .object(error$details)
        value
      }, error = function(invalid) {
        cat(conditionMessage(error), "\n", file = stderr())
        list(code = "operation_failed", message = paste0("Worker callback or output serialization failed: ", conditionMessage(error)))
      })
      list(id = request$id, ok = FALSE, error = payload)
    })
    encode <- function(response) {
      frame <- c(charToRaw(enc2utf8(codec$encode(response))), as.raw(10L))
      if (length(frame) > 1024L * 1024L) stop("Worker response exceeds the protocol limit.")
      frame
    }
    frame <- tryCatch(encode(response), error = function(error) {
      success <<- FALSE
      failure <- list(id = request$id, ok = FALSE, error = list(code = "operation_failed",
                       message = paste0("Worker output serialization failed: ", conditionMessage(error))))
      tryCatch(encode(failure), error = function(error) {
        encode(list(id = request$id, ok = FALSE,
               error = list(code = "internal_error", message = "Worker response exceeds the protocol limit.")))
      })
    })
    writeBin(frame, connection)
    if (!ready || (request$method == "shutdown" && success)) return(invisible(NULL))
  }
}
