test_that("serving requires a CLI-supplied adapter", {
  withr::local_envvar(METEWAND_ADAPTER_PATH = NA_character_)
  expect_error(serve_implementation(), "Metewand CLI")
})

test_that("serving forwards current and future callbacks to the adapter", {
  adapter <- withr::local_tempfile(fileext = ".R")
  writeLines(
    c("binding_api_version <- 1L", "serve_bindings <- function(...) list(...)"),
    adapter
  )
  withr::local_envvar(METEWAND_ADAPTER_PATH = adapter)
  callback <- function(state, context) NULL
  call <- serve_implementation(
    prepare = function(context) NULL,
    execute = callback,
    future_callback = callback
  )

  expect_identical(call[[1L]], 1L)
  expect_identical(call[[3L]]$future_callback, callback)
})

test_that("serving rejects incompatible binding API versions", {
  adapter <- withr::local_tempfile(fileext = ".R")
  writeLines("binding_api_version <- 2L", adapter)
  withr::local_envvar(METEWAND_ADAPTER_PATH = adapter)

  expect_error(serve_implementation(), "binding API")
})
