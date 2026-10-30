use crate::protocol::PrepareRequest;
use crate::runtime::worker_sdk::{Implementation, WorkerError, WorkerResult, serve_implementation};
use serde_json::Value;

struct Solver;

impl Implementation for Solver {
    type Output = Value;

    fn prepare(&mut self, _request: &PrepareRequest) -> WorkerResult<()> {
        // TODO: Load the dataset, translate parameters, and initialize solver state.
        Err(WorkerError::operation_failed(
            "TODO: Implement scientific preparation.",
        ))
    }

    fn execute(&mut self) -> WorkerResult<Value> {
        // TODO: Run the method and return scientific result data.
        Err(WorkerError::operation_failed(
            "TODO: Implement the method and result conversion.",
        ))
    }
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_implementation(&mut Solver)?;
    Ok(())
}
