.safe_integer <- 9007199254740991
.is_integer <- function(value, maximum = .safe_integer) {
  (is.integer(value) || is.double(value)) && length(value) == 1L &&
    is.null(attributes(value)) && is.finite(value) && value >= 0 &&
    value <= maximum && value == trunc(value)
}
.is_string <- function(value) {
  (is.character(value) && length(value) == 1L && !is.na(value) && is.null(attributes(value))) ||
    inherits(value, "metewand_string")
}
.nonempty_string <- function(value) {
  .is_string(value) && if (is.raw(value)) length(value) > 0L else nzchar(value)
}
.is_object <- function(value) {
  is.list(value) && ((!is.object(value) && !is.null(names(value))) ||
                      inherits(value, "metewand_object"))
}
.object <- function(value, name) {
  if (!.is_object(value)) stop(name, " must be a JSON object.", call. = FALSE)
  canonical_bytes(value)
  value
}
.ordinary_object <- function(value, name) {
  .object(value, name)
  if (is.object(value)) stop(name, " has unsupported fields.", call. = FALSE)
  value
}
.relative_path <- function(value) {
  if (!is.character(value) || length(value) != 1L || is.na(value) ||
      !is.null(attributes(value)) || !nzchar(value) || startsWith(value, "/") ||
      grepl("\\", value, fixed = TRUE) || endsWith(value, "/") ||
      any(strsplit(value, "/", fixed = TRUE)[[1L]] %in% c("", ".", ".."))) {
    stop("Paths must be normalized, nonempty, and relative to the assigned directory.", call. = FALSE)
  }
  canonical_bytes(value)
  value
}
.output_path <- function(directory, relative) {
  relative <- .relative_path(relative)
  root <- normalizePath(directory, mustWork = TRUE)
  target <- file.path(root, relative)
  link <- Sys.readlink(target)
  if (!file.exists(target) && !is.na(link) && nzchar(link)) {
    stop("Output paths cannot identify dangling symlinks.", call. = FALSE)
  }
  parent <- normalizePath(dirname(target), mustWork = TRUE)
  resolved <- if (file.exists(target)) normalizePath(target, mustWork = TRUE) else file.path(parent, basename(target))
  if (!startsWith(resolved, paste0(sub("/+$", "", root), "/"))) {
    stop("Output paths must stay beneath the assigned directory.", call. = FALSE)
  }
  target
}

read_json <- function(path) {
  connection <- file(path, "rb")
  on.exit(close(connection))
  parse_json(readBin(connection, "raw", n = file.info(path)$size))
}

write_json <- function(path, value) {
  bytes <- canonical_bytes(value)
  connection <- file(path, "wb")
  on.exit(close(connection))
  writeBin(bytes, connection)
  invisible(NULL)
}

.representation <- function(value) {
  .ordinary_object(value, "File representation")
  if (!length(value) || any(!names(value) %in% c("shape", "data_type", "byte_order", "sparse_layout"))) {
    stop("File representation must contain only version-1 representation fields.", call. = FALSE)
  }
  if ("shape" %in% names(value) &&
      (!is.list(value$shape) || !is.null(attributes(value$shape)) ||
       !all(vapply(value$shape, .is_integer, logical(1))))) {
    stop("Shape must contain nonnegative safe integers.", call. = FALSE)
  }
  for (key in intersect(c("data_type", "sparse_layout"), names(value))) {
    if (!.nonempty_string(value[[key]])) stop(key, " must be a nonempty string.", call. = FALSE)
  }
  if ("byte_order" %in% names(value) &&
      (!is.character(value$byte_order) || !value$byte_order %in% c("little", "big", "not_applicable"))) {
    stop("Invalid byte order.", call. = FALSE)
  }
  invisible(NULL)
}

.files <- function(files, manifest = NULL) {
  if (!is.list(files) || !is.null(attributes(files))) stop("Files must be an unnamed list.", call. = FALSE)
  paths <- character()
  for (entry in files) {
    .ordinary_object(entry, "File entry")
    required <- c("path", "media_type", "sha256", "size_bytes")
    if (!all(required %in% names(entry)) || any(!names(entry) %in% c(required, "representation"))) {
      stop("File entries must contain the version-1 inventory fields.", call. = FALSE)
    }
    path <- .relative_path(entry$path)
    if (path %in% paths || identical(path, manifest)) {
      stop("Inventory paths must be unique and exclude the manifest itself.", call. = FALSE)
    }
    paths <- c(paths, path)
    if (!.nonempty_string(entry$media_type)) stop("Media types must be nonempty strings.", call. = FALSE)
    if (!is.character(entry$sha256) || !grepl("^[0-9a-f]{64}$", entry$sha256)) {
      stop("File digests must be lowercase SHA-256 hex strings.", call. = FALSE)
    }
    if (!.is_integer(entry$size_bytes)) stop("File sizes must be nonnegative safe integers.", call. = FALSE)
    if ("representation" %in% names(entry)) .representation(entry$representation)
  }
  # Radix ordering uses UTF-8 bytes and does not depend on the worker's locale.
  files[order(enc2utf8(paths), method = "radix")]
}

file_entry <- function(directory, path, media_type, representation = NULL) {
  path <- .relative_path(path)
  target <- .output_path(directory, path)
  if (!file_test("-f", target) || nzchar(Sys.readlink(target))) {
    stop("An inventory entry must identify a regular file.", call. = FALSE)
  }
  entry <- list(path = path, media_type = media_type,
                sha256 = digest::digest(target, algo = "sha256", file = TRUE),
                size_bytes = file.info(target)$size)
  if (!is.null(representation)) entry$representation <- representation
  .files(list(entry))[[1L]]
}

write_dataset_manifest <- function(directory, files, manifest = "dataset-manifest.json") {
  manifest <- .relative_path(manifest)
  write_json(.output_path(directory, manifest),
             list(schema_version = 1, complete = TRUE, files = .files(files, manifest)))
  manifest
}

write_result <- function(directory, data, schema, files = list(), manifest = "result.json") {
  manifest <- .relative_path(manifest)
  write_json(.output_path(directory, manifest), list(
    schema_version = 1, schema = .relative_path(schema),
    data = .object(data, "Result data"), files = .files(files, manifest)))
  manifest
}

write_metrics <- function(path, data, schema) {
  write_json(path, list(schema_version = 1, schema = .relative_path(schema),
                       data = .object(data, "Metrics data")))
}
