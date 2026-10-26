use metewand_protocol::MaterializeRequest;
use metewand_runtime::worker_sdk::{WorkerError, WorkerResult, serve_dataset_materializer};

fn materialize(_request: &MaterializeRequest) -> WorkerResult<String> {
    // TODO: Construct the dataset and write files into request.output_dir().
    // Use write_json and write_dataset_manifest, returning the relative manifest
    // path after closing every file.
    Err(WorkerError::operation_failed(
        "TODO: Implement dataset construction and conversion.",
    ))
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_dataset_materializer(&mut materialize)?;
    Ok(())
}
