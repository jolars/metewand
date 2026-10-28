test_that("scientific output constructors describe callback values", {
  expect_s3_class(result(list(answer = 1)), "metewand_result")
  expect_s3_class(
    dataset(files = list(artifact("data.bin", "application/octet-stream"))),
    "metewand_dataset"
  )
  expect_identical(
    evaluation(list(error = 0), "not_required")$verdict,
    "not_required"
  )
})
