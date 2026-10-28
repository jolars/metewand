test_that("JSON round trips preserve binary64 values and container shapes", {
  path <- withr::local_tempfile()
  value <- list(
    number = 1.0000000000000002,
    maximum = 9007199254740991,
    small = 5e-324,
    text = "é",
    empty = json_object(),
    array = list(),
    singleton = json_array(1),
    nothing = NULL
  )
  write_json(path, value)
  decoded <- read_json(path)

  expect_identical(decoded$number, value$number)
  expect_identical(decoded$maximum, value$maximum)
  expect_identical(decoded$small, value$small)
  expect_identical(decoded$text, value$text)
  expect_identical(names(decoded$empty), character())
  expect_null(names(decoded$array))
  expect_type(decoded$singleton, "list")
  expect_length(decoded$singleton, 1L)
  expect_equal(decoded$singleton[[1L]], 1)
  expect_true("nothing" %in% names(decoded))
})

test_that("JSON writing rejects unsupported values", {
  path <- withr::local_tempfile()
  for (value in list(
    NA,
    Inf,
    2^53,
    as.Date("2020-01-01"),
    as.raw(1),
    matrix(1),
    stats::setNames(list(1, 2), c("a", "a"))
  )) {
    expect_error(write_json(path, value), "JSON|Convert|Missing")
  }
})

test_that("JSON objects preserve empty keys", {
  path <- withr::local_tempfile()
  write_json(path, stats::setNames(list(list(value = 1)), ""))
  expect_identical(names(read_json(path)), "")
})

test_that("JSON reading rejects embedded NULs and duplicate keys", {
  path <- withr::local_tempfile()
  for (source in c('"a\\u0000b"', '{"a":1,"a":2}')) {
    writeLines(source, path)
    expect_error(read_json(path), "NUL|unique|UTF-8")
  }
})

test_that("JSON reading rejects BOMs and comments", {
  path <- withr::local_tempfile()
  for (source in c(
    paste0("\ufeff", '{"value":1}'),
    '/*comment*/{"value":1}',
    '//comment\n{"value":1}',
    '{"value":/*comment*/1}',
    '{"value":1/*comment*/}',
    '{"value":1}/*comment*/',
    '{"value":1}//comment'
  )) {
    writeLines(source, path)
    expect_error(read_json(path), "JSON")
  }
})

test_that("JSON reading rejects lone surrogates", {
  path <- withr::local_tempfile()
  for (source in c(
    r"("\ud800")",
    r"("\uDBFF")",
    r"("\udfff")",
    r"("\uDC00")",
    r"("\ud800x")",
    r"("\ud800\u0041")",
    r"("\ud800\ud800")",
    r"("\udc00\ud800")",
    r"("\ud800\\udc00")",
    r"({"\ud800":1})",
    r"({"nested":["\ud800"]})",
    r"(["\ud800","\udc00"])",
    r"("\ud800\udc00\udc00")",
    r"("\\ud800\udc00")"
  )) {
    writeLines(source, path)
    expect_error(read_json(path), "surrogate|UTF-8")
  }
})

test_that("JSON reading decodes valid surrogate pairs", {
  path <- withr::local_tempfile()
  sources <- c(r"("\ud800\udc00")", r"("\uDBFF\uDFFF")", r"("\\\uD83D\uDE00")")
  expected <- c("\U00010000", "\U0010ffff", paste0("\\", "\U0001f600"))
  for (i in seq_along(sources)) {
    writeLines(sources[[i]], path)
    expect_identical(read_json(path), expected[[i]])
  }

  writeLines(r"({"\uD83D\uDE00":"\ud83d\ude00"})", path)
  decoded <- read_json(path)
  expect_identical(names(decoded), "\U0001f600")
  expect_identical(decoded[[1L]], "\U0001f600")
})

test_that("JSON round trips preserve literal escapes and comment text", {
  path <- withr::local_tempfile()
  for (text in c(
    r"(literal \ud800)",
    r"(literal \udfff)",
    r"(literal \\ud800\udc00)",
    r"(literal \u0000)",
    "\ufeff",
    "https://example.invalid/*comment*/ //comment"
  )) {
    write_json(path, list(text = text))
    expect_identical(read_json(path)$text, text)
  }
})
