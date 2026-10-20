# Seed derivation stays in this test harness because the orchestrator owns seeds.
fixture <- function(path) {
  value <- read_json(file.path(root, "fixtures", path))
  stopifnot(value$version == 1)
  value
}
sha256 <- function(bytes) digest::digest(bytes, algo = "sha256", serialize = FALSE)
hex_bytes <- function(hex) {
  starts <- seq.int(1L, nchar(hex), by = 2L)
  as.raw(strtoi(substring(hex, starts, starts + 1L), base = 16L))
}
check_identity <- function(kind, source, identity) {
  bytes <- canonical_bytes(parse_json(source))
  stopifnot(identical(bytes, charToRaw(enc2utf8(source))),
    paste0("mw1-", kind, "-", sha256(c(charToRaw(kind), as.raw(0), bytes))) == identity)
}
for (vector in fixture("identities/v1.json")$vectors) check_identity(vector$kind, vector$canonical, vector$identity)
for (vector in fixture("identities/diagnostics-v1.json")$vectors) {
  diagnostic <- vector$diagnostic
  if (!"details" %in% names(diagnostic)) diagnostic["details"] <- list(NULL)
  stopifnot(text(list(value = diagnostic, version = 1)) == vector$canonical)
  check_identity("diagnostic", vector$canonical, vector$id)
}
policies <- read_json(file.path(root, "fixtures/identities/concurrency-v1.json"))
stopifnot(policies$identity_version == 1)
for (vector in policies$policies) check_identity("execution-policy", vector$canonical, vector$identity)
derivation <- paste(readLines(file.path(root, "fixtures/identities/dataset-derivation-v1.json")), collapse = "\n")
check_identity("dataset-derivation", derivation,
  "mw1-dataset-derivation-875a677f1d0ce2b6b77bc5a03f213774188e0ab45f4b8fc18cbf754de70847c4")
seeds <- fixture("seeds/v1.json")
fields <- list(
  dataset = c("dataset_definition", "dataset_parameters", "experiment_seed"),
  implementation = c("dataset_configuration", "experiment_seed", "implementation_repetition", "problem_configuration"),
  scheduling = c("benchmark_name", "experiment_name", "experiment_seed", "scheduling_policy_version"))
for (role in names(fields)) {
  for (vector in seeds[[role]]) {
    bytes <- canonical_bytes(vector[fields[[role]]])
    digest <- sha256(c(charToRaw(seeds$domain), as.raw(0), charToRaw(role), as.raw(0), bytes))
    leading <- as.double(hex_bytes(digest)[1:7])
    value <- sum(leading[1:6] * 256^(5:0)) * 32 + floor(leading[7] / 8)
    stopifnot(identical(bytes, charToRaw(vector$canonical_fields)), digest == vector$digest, value == vector$value)
  }
}

with_bindings <- function(bindings, expr) {
  namespace <- asNamespace("metewand")
  previous <- mget(names(bindings), envir = namespace)
  replace_bindings <- function(values) {
    for (name in names(values)) {
      unlockBinding(name, namespace)
      assign(name, values[[name]], envir = namespace)
      lockBinding(name, namespace)
    }
  }
  on.exit(replace_bindings(previous))
  replace_bindings(bindings)
  force(expr)
}

golden <- fixture("worker-protocol/v1/golden.json")
for (session in golden$sessions) {
  directory <- tempfile()
  dir.create(directory)
  callbacks <- switch(session$role,
    implementation = list(
      prepare = function(dataset, contract, problem, implementation, seed) list(seed = seed),
      execute = function(state, budget, control) {
        stopifnot(is.null(budget), is.null(control))
        state
      },
      statistics = function(state) list(seed = state$seed),
      result = function(state, directory) write_result(directory, list(answer = 6), schema = "schemas/result.json"),
      reset = function(state) NULL),
    dataset_materializer = function(source, parameters, seed, output) write_dataset_manifest(output, list()),
    problem_evaluator = function(dataset, contract, parameters, result, metrics) {
      write_metrics(metrics, list(error = 0), schema = "schemas/metrics.json")
      parameters$verdict
    })
  requests <- lapply(session$exchanges, function(step) {
    parse_json(gsub("@ROOT@", directory, step$request, fixed = TRUE))
  })
  ticks <- c(100, 125)
  clock <- function() {
    tick <- ticks[[1L]]
    ticks <<- ticks[-1L]
    tick
  }
  encode <- metewand:::.encode_response
  inspect <- function(response) {
    if (response$ok) {
      if (!is.null(response$result)) {
        manifest <- read_json(file.path(directory, response$result$manifest))
        stopifnot(manifest$data$answer == 6)
      }
      if (!is.null(response$dataset)) read_json(file.path(directory, response$dataset$manifest))
      if (!is.null(response$verdict)) read_json(file.path(directory, "metrics.json"))
    }
    encode(response)
  }
  responses <- with_bindings(list(.clock = clock, .encode_response = inspect),
    exchange(callbacks, session$role, requests))
  for (i in seq_along(responses)) {
    expected <- gsub("@SDK@", "metewand-r", session$exchanges[[i]]$response, fixed = TRUE)
    stopifnot(identical(c(canonical_bytes(responses[[i]]), as.raw(10)), charToRaw(enc2utf8(expected))))
  }
  unlink(directory, recursive = TRUE)
}
for (vector in golden$responses) {
  stopifnot(identical(metewand:::.encode_response(parse_json(vector$frame)), charToRaw(vector$frame)))
}
for (vector in golden$rejected_frames) {
  input <- rawConnection(hex_bytes(vector$hex))
  output <- rawConnection(raw(), "wb")
  fails(metewand:::.serve(list(), "implementation", input, output), "metewand_protocol_error")
  close(input)
  close(output)
}
errors <- fixture("worker-protocol/v1/errors.json")
for (vector in errors$accepted) {
  callback <- function(...) stop(do.call(worker_error, vector$error))
  responses <- exchange(callback, "dataset_materializer", list(
    hello("dataset_materializer"),
    list(id = "failure", method = "materialize", source_dir = "/source", dataset_parameters = json_object(),
         dataset_seed = 0, output_dir = "/output"),
    request("shutdown")))
  stopifnot(identical(c(canonical_bytes(responses[[2L]]), as.raw(10)), charToRaw(enc2utf8(vector$frame))))
}
for (vector in errors$rejected) fails(do.call(worker_error, vector))

directory <- tempfile()
dir.create(directory)
events <- character()
clock <- function() {
  events <<- c(events, "clock")
  stopifnot(get("state", envir = parent.frame(), inherits = TRUE)$answer == 6)
  if (sum(events == "clock") == 1L) 100 else 125
}
callbacks <- list(
  prepare = function(...) { events <<- c(events, "prepare"); list(answer = 6) },
  execute = function(state, ...) { events <<- c(events, "execute"); state$answer <- 7; state },
  statistics = function(state) { events <<- c(events, "statistics"); list(count = 3) },
  result = function(state, directory) {
    events <<- c(events, "result")
    write_json(file.path(directory, "answer.json"), state)
    write_result(directory, state, schema = "schemas/result.json",
      files = list(file_entry(directory, "answer.json", "application/json")))
  })
encode <- metewand:::.encode_response
inspect <- function(response) {
  if (!is.null(response$result)) {
    events <<- c(events, "reply")
    manifest <- read_json(file.path(directory, response$result$manifest))
    stopifnot(manifest$data$answer == 7,
      read_json(file.path(directory, manifest$files[[1]]$path))$answer == 7,
      identical(canonical_bytes(file_entry(directory, "answer.json", "application/json")),
                canonical_bytes(manifest$files[[1]])))
  }
  encode(response)
}
responses <- with_bindings(list(.clock = clock, .encode_response = inspect),
  exchange(callbacks, "implementation", list(hello(),
    request("prepare", dataset_dir = directory, problem_contract_id = "c", problem_parameters = json_object(),
            implementation_parameters = json_object(), implementation_seed = 23),
    request("execute", result_dir = directory), request("shutdown"))))
stopifnot(identical(events, c("prepare", "clock", "execute", "clock", "statistics", "result", "reply")),
          responses[[3]]$implementation_time_ns == 25)
unlink(directory, recursive = TRUE)
