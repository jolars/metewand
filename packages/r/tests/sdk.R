library(metewand)

fails <- function(expr, pattern) {
  error <- tryCatch({ force(expr); NULL }, error = identity)
  stopifnot(inherits(error, "error"), grepl(pattern, conditionMessage(error)))
}
stopifnot(inherits(result(list(answer = 1)), "metewand_result"),
          inherits(dataset(files = list(artifact("data.bin", "application/octet-stream"))), "metewand_dataset"),
          evaluation(list(error = 0), "not_required")$verdict == "not_required")
path <- tempfile()
value <- list(number = 1.0000000000000002, maximum = 9007199254740991,
              small = 5e-324, text = "é", empty = json_object(), array = list(),
              singleton = json_array(1), nothing = NULL)
write_json(path, value)
decoded <- read_json(path)
stopifnot(identical(decoded$number, value$number), identical(decoded$maximum, value$maximum),
          identical(decoded$small, value$small), identical(decoded$text, value$text),
          identical(names(decoded$empty), character()), is.null(names(decoded$array)),
          is.list(decoded$singleton), length(decoded$singleton) == 1L, decoded$singleton[[1L]] == 1, "nothing" %in% names(decoded))
for (value in list(NA, Inf, 2^53, as.Date("2020-01-01"), as.raw(1), matrix(1),
                   stats::setNames(list(1, 2), c("a", "a")))) {
  fails(write_json(path, value), "JSON|Convert|Missing")
}
write_json(path, stats::setNames(list(list(value = 1)), ""))
stopifnot(identical(names(read_json(path)), ""))
for (source in c('"a\\u0000b"', '{"a":1,"a":2}')) {
  writeLines(source, path)
  fails(read_json(path), "NUL|unique|UTF-8")
}
for (source in c(paste0("\ufeff", '{"value":1}'),
                 '/*comment*/{"value":1}', '//comment\n{"value":1}',
                 '{"value":/*comment*/1}', '{"value":1/*comment*/}',
                 '{"value":1}/*comment*/', '{"value":1}//comment')) {
  writeLines(source, path)
  fails(read_json(path), "JSON")
}
for (source in c(r"("\ud800")", r"("\uDBFF")", r"("\udfff")",
                 r"("\uDC00")", r"("\ud800x")", r"("\ud800\u0041")",
                 r"("\ud800\ud800")", r"("\udc00\ud800")",
                 r"("\ud800\\udc00")", r"({"\ud800":1})",
                 r"({"nested":["\ud800"]})", r"(["\ud800","\udc00"])",
                 r"("\ud800\udc00\udc00")", r"("\\ud800\udc00")")) {
  writeLines(source, path)
  fails(read_json(path), "surrogate|UTF-8")
}
sources <- c(r"("\ud800\udc00")", r"("\uDBFF\uDFFF")", r"("\\\uD83D\uDE00")")
expected <- c("\U00010000", "\U0010ffff", paste0("\\", "\U0001f600"))
for (i in seq_along(sources)) {
  writeLines(sources[[i]], path)
  stopifnot(read_json(path) == expected[[i]])
}
writeLines(r"({"\uD83D\uDE00":"\ud83d\ude00"})", path)
decoded <- read_json(path)
stopifnot(identical(names(decoded), "\U0001f600"), decoded[[1L]] == "\U0001f600")
for (text in c(r"(literal \ud800)", r"(literal \udfff)",
               r"(literal \\ud800\udc00)", r"(literal \u0000)",
               "\ufeff", "https://example.invalid/*comment*/ //comment")) {
  write_json(path, list(text = text))
  stopifnot(read_json(path)$text == text)
}
Sys.unsetenv("METEWAND_ADAPTER_PATH")
fails(serve_implementation(), "Metewand CLI")
adapter <- tempfile(fileext = ".R")
writeLines(c("binding_api_version <- 1L", "serve_bindings <- function(...) list(...)"), adapter)
Sys.setenv(METEWAND_ADAPTER_PATH = adapter)
callback <- function(state, context) NULL
call <- serve_implementation(prepare = function(context) NULL, execute = callback, future_callback = callback)
stopifnot(call[[1L]] == 1L, identical(call[[3L]]$future_callback, callback))
writeLines("binding_api_version <- 2L", adapter)
fails(serve_implementation(), "binding API")
Sys.unsetenv("METEWAND_ADAPTER_PATH")
unlink(c(path, adapter))
