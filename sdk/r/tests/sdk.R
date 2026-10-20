library(metewand)

fails <- function(expr, class = "error") {
  error <- tryCatch({ force(expr); NULL }, error = identity)
  stopifnot(inherits(error, class))
  invisible(error)
}
text <- function(value) rawToChar(canonical_bytes(value))
root <- Sys.getenv("METEWAND_REPOSITORY", "../../..")
vectors <- read_json(file.path(root, "fixtures/canonical-json/v1.json"))
for (vector in vectors$accepted) {
  stopifnot(identical(text(parse_json(vector$input)), vector$canonical))
}
for (vector in vectors$rejected) fails(parse_json(vector$input), "metewand_canonical_error")

stopifnot(text(list()) == "[]", text(json_object()) == "{}")
stopifnot(text(list(NULL, TRUE, -0, 1e-400)) == "[null,true,0,0]")
stopifnot(text(parse_json('"a\\u0000b"')) == '"a\\u0000b"')
stopifnot(text(parse_json('{"a\\u0000b":null,"":1}')) == '{"":1,"a\\u0000b":null}')
stopifnot(text(json_object(list(1, 2), keys = list("", json_string(as.raw(0))))) ==
            '{"":1,"\\u0000":2}')
stopifnot(parse_json('{"a\\u0000b":null,"value":1}')$value == 1)
fails(parse_json('{"a\\u0000":1,"a\\u0000":2}'), "metewand_canonical_error")
for (value in list(NA, NA_real_, NA_character_, NaN, Inf, 2^53,
                   c(1, 2), numeric(), factor("a"), matrix(1), as.Date("2020-01-01"),
                   new.env(), as.raw(1), structure(1, names = "x"),
                   setNames(list(1, 2), c("a", "a")))) {
  fails(canonical_bytes(value), "metewand_canonical_error")
}
for (input in list(as.raw(c(34, 255, 34)), as.raw(c(239, 187, 191, 123, 125)),
                   '"\\udfff"', '"\\ud800x"', '"\\u0000" true', '[1,]',
                   '{"a":}', '1.', '+1', '1e', '"a\nb"')) {
  fails(parse_json(input), "metewand_canonical_error")
}

directory <- tempfile()
dir.create(directory)
write_json(file.path(directory, "data.json"), list(values = list(1, 2, 3)))
entry <- file_entry(directory, "data.json", "application/json",
                    representation = list(shape = list(3), data_type = "json"))
stopifnot(entry$size_bytes == 18,
          entry$sha256 == "abe0a9abcc957f9f5aa04d68c4f49fadee7c4b62343e403a3a1c0b646519d53c")
manifest <- write_dataset_manifest(directory, list(entry))
stopifnot(read_json(file.path(directory, manifest))$complete)
manifest <- write_result(directory, list(answer = 6), schema = "schemas/result.json")
stopifnot(text(read_json(file.path(directory, manifest))) ==
            '{"data":{"answer":6},"files":[],"schema":"schemas/result.json","schema_version":1}')
for (path in c("../escape", "/absolute", "a/./b", "a//b", "a\\b", "")) {
  fails(file_entry(directory, path, "text/plain"))
}
fails(write_dataset_manifest(directory, list(entry, entry)))
fails(file_entry(directory, "data.json", "text/plain", representation = list(shape = list(-1))))
fails(write_result(directory, list(), schema = "s.json"))
outside <- tempfile()
dir.create(outside)
stopifnot(file.symlink(outside, file.path(directory, "link")))
fails(write_result(directory, json_object(), schema = "s.json", manifest = "link/escape.json"))
stopifnot(file.symlink(file.path(outside, "missing"), file.path(directory, "dangling")))
fails(write_result(directory, json_object(), schema = "s.json", manifest = "dangling"))

hello <- function(role = "implementation") list(
  id = "0", method = "hello", protocols = list(1), role = role,
  worker_id = paste0("mw1-resolved-worker-", strrep("a", 64)))
request <- function(method, ...) c(list(id = "1", method = method), list(...))
exchange <- function(worker, role, requests) {
  bytes <- do.call(c, lapply(requests, function(r) c(canonical_bytes(r), as.raw(10))))
  input <- rawConnection(bytes)
  output <- rawConnection(raw(), "wb")
  on.exit({close(input); close(output)})
  metewand:::.serve(worker, role, input, output)
  lines <- strsplit(rawToChar(rawConnectionValue(output)), "\n", fixed = TRUE)[[1]]
  lapply(lines, function(line) {
    value <- parse_json(line)
    stopifnot(text(value) == line)
    value
  })
}
prepare_request <- request("prepare", dataset_dir = directory, problem_contract_id = "c",
                           problem_parameters = list(offset = 2),
                           implementation_parameters = json_object(), implementation_seed = 23)
events <- character()
worker <- list(
  prepare = function(dataset, contract, problem, implementation, seed) {
    events <<- c(events, "prepare")
    list(answer = sum(unlist(read_json(file.path(dataset, "data.json"))$values)) + problem$offset,
         seed = seed)
  },
  execute = function(state, budget, observation_control) {
    stopifnot(is.null(budget), is.null(observation_control))
    events <<- c(events, "execute")
    state
  },
  result = function(state, directory) {
    events <<- c(events, "result")
    write_result(directory, list(answer = state$answer), schema = "schemas/result.json")
  },
  reset = function(state) { events <<- c(events, "reset"); NULL },
  shutdown = function(state) { events <<- c(events, "shutdown"); NULL }
)
responses <- exchange(worker, "implementation", list(hello(), prepare_request,
  request("execute", result_dir = directory), request("reset"),
  request("execute", result_dir = directory), request("shutdown")))
stopifnot(identical(events, c("prepare", "execute", "result", "reset", "shutdown")),
          responses[[1]]$sdk$name == "metewand-r",
          text(responses[[1]]$capabilities) == '["one_shot"]',
          responses[[3]]$ok, text(responses[[3]]$statistics) == "{}",
          is.numeric(responses[[3]]$implementation_time_ns),
          responses[[5]]$error$code == "invalid_request",
          read_json(file.path(directory, "result.json"))$data$answer == 8)
stopifnot(text(responses[[1]]) == paste0(
  '{"capabilities":["one_shot"],"id":"0","ok":true,"protocol":1,',
  '"sdk":{"name":"metewand-r","version":"0.1.0"},"worker_id":"',
  hello()$worker_id, '"}'))
greeting <- hello()
greeting$id <- json_string(as.raw(c(97, 0, 98)))
responses <- exchange(worker, "implementation", list(greeting, request("shutdown")))
stopifnot(text(responses[[1]]$id) == '"a\\u0000b"')
bad <- parse_json('{"id":"1","method":"reset","a\\u0000":1}')
responses <- exchange(worker, "implementation", list(hello(), bad, request("shutdown")))
stopifnot(responses[[2]]$error$code == "invalid_request")

materialize_request <- request("materialize", source_dir = directory,
  dataset_parameters = list(values = list(1)), dataset_seed = 17, output_dir = directory)
materialize <- function(source, parameters, seed, output) {
  stopifnot(source == directory, seed == 17)
  write_json(file.path(output, "data.json"), parameters)
  write_dataset_manifest(output, list(file_entry(output, "data.json", "application/json")))
}
responses <- exchange(materialize, "dataset_materializer", list(
  hello("dataset_materializer"), materialize_request, request("shutdown")))
stopifnot(responses[[2]]$dataset$manifest == "dataset-manifest.json")
for (callback in list(function(...) stop("unexpected failure"), function(...) "missing.json")) {
  responses <- exchange(callback, "dataset_materializer", list(
    hello("dataset_materializer"), materialize_request, request("shutdown")))
  stopifnot(responses[[2]]$error$code == "operation_failed", responses[[3]]$ok)
}
evaluate_request <- request("evaluate", dataset_dir = directory, problem_contract_id = "c",
  problem_parameters = json_object(), result_dir = directory,
  metrics_path = file.path(directory, "metrics.json"))
responses <- exchange(function(...) "reached", "problem_evaluator", list(
  hello("problem_evaluator"), evaluate_request, request("shutdown")))
stopifnot(responses[[2]]$error$code == "operation_failed")
for (verdict in c("invalid", "reached", "not_reached", "not_required", "invented")) {
  evaluate <- function(dataset, contract, parameters, result, metrics) {
    write_metrics(metrics, list(error = 0), schema = "schemas/metrics.json")
    verdict
  }
  responses <- exchange(evaluate, "problem_evaluator", list(
    hello("problem_evaluator"), evaluate_request, request("shutdown")))
  if (verdict == "invented") stopifnot(responses[[2]]$error$code == "operation_failed")
  else stopifnot(responses[[2]]$verdict == verdict)
}
for (code in c("invalid_request", "unsupported_operation", "operation_failed", "internal_error")) {
  fail <- function(...) stop(worker_error(code, "bad data", list(index = 2)))
  responses <- exchange(fail, "dataset_materializer", list(
    hello("dataset_materializer"), materialize_request, request("shutdown")))
  stopifnot(text(responses[[2]]$error) == text(list(code = code, message = "bad data", details = list(index = 2))))
}
fails(worker_error("invented", "bad"))
for (bad in list(request("execute", result_dir = directory, budget = json_object()),
                 request("reset", extra = TRUE), request("shutdown", extra = TRUE),
                 request("execute_stream"), materialize_request, hello(),
                 replace(prepare_request, "implementation_seed", list(TRUE)))) {
  responses <- exchange(worker, "implementation", list(hello(), bad, request("shutdown")))
  stopifnot(!responses[[2]]$ok)
}
for (change in list(list(role = "problem_evaluator"), list(protocols = list(2)),
                    list(protocols = list(1, 1)), list(worker_id = "wrong"), list(extra = 1))) {
  greeting <- hello()
  greeting[names(change)] <- change
  responses <- exchange(worker, "implementation", list(greeting))
  stopifnot(length(responses) == 1L, !responses[[1]]$ok)
}
for (bytes in list(charToRaw('{"id":"1","id":"2"}\n'), charToRaw("{}"),
                   charToRaw('{"identifier":"1","method":"hello"}\n'),
                   as.raw(c(255, 10)), charToRaw("{}\n"), charToRaw("NaN\n"),
                   rep(as.raw(120), 1024 * 1024 + 1))) {
  input <- rawConnection(bytes)
  output <- rawConnection(raw(), "wb")
  fails(metewand:::.serve(worker, "implementation", input, output), "metewand_protocol_error")
  close(input)
  close(output)
}
for (failing in c("prepare", "reset", "execute", "result")) {
  broken <- worker
  broken[[failing]] <- function(...) stop(worker_error("operation_failed", "broken"))
  calls <- list(hello(), prepare_request)
  if (failing == "reset") calls <- c(calls, list(request("reset")))
  if (failing %in% c("execute", "result")) calls <- c(calls, list(request("execute", result_dir = directory)))
  calls <- c(calls, list(request("execute", result_dir = directory), request("shutdown")))
  responses <- exchange(broken, "implementation", calls)
  stopifnot(responses[[length(responses) - 1]]$error$code == "invalid_request")
}
broken <- worker
broken$reset <- NULL
responses <- exchange(broken, "implementation", list(hello(), prepare_request,
  request("reset"), request("execute", result_dir = directory), request("shutdown")))
stopifnot(responses[[3]]$error$code == "unsupported_operation",
          responses[[4]]$error$code == "invalid_request")

limit <- 1024 * 1024
frame <- metewand:::.encode_response(list(text = ""))
stopifnot(length(metewand:::.encode_response(list(text = strrep("a", limit - length(frame))))) == limit)
fails(metewand:::.encode_response(list(text = strrep("a", limit))))
greeting <- canonical_bytes(hello())
padding <- rep(as.raw(32), limit - length(greeting) - 1L)
source <- c(greeting, padding, as.raw(10), canonical_bytes(request("shutdown")), as.raw(10))
input <- rawConnection(source)
output <- rawConnection(raw(), "wb")
metewand:::.serve(worker, "implementation", input, output)
stopifnot(length(strsplit(rawToChar(rawConnectionValue(output)), "\n", fixed = TRUE)[[1L]]) == 2L)
close(input)
close(output)
fail <- function(...) stop(worker_error("operation_failed", "large", list(text = strrep("x", limit))))
responses <- exchange(fail, "dataset_materializer", list(
  hello("dataset_materializer"), materialize_request, request("shutdown")))
stopifnot(responses[[2]]$error$code == "internal_error")
fails(metewand:::.encode_failure(strrep("i", limit), worker_error("operation_failed", "bad")),
      "metewand_protocol_error")
fails(serve_implementation(NULL, function(...) NULL, function(...) NULL))
fails(serve_dataset_materializer(NULL))
fails(serve_evaluator(NULL))
fails(serve_evaluator(function(...) NULL, read_fd = 1, write_fd = 1))
fails(serve_evaluator(function(...) NULL, read_fd = -1, write_fd = 1))
unlink(c(directory, outside), recursive = TRUE)
cat("R SDK contracts passed.\n")
