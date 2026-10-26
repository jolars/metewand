use metewand_protocol::EvaluateRequest;
use metewand_runtime::worker_sdk::{Evaluation, WorkerError, WorkerResult, serve_evaluator};

fn evaluate(_request: &EvaluateRequest) -> WorkerResult<Evaluation> {
    // TODO: Read the dataset and result with read_json and compute independent metrics.
    // Read validity and completion targets from request.problem_semantics(). Return
    // Evaluation::new(metrics, verdict) with an explicit EvaluationVerdict.
    Err(WorkerError::operation_failed(
        "TODO: Implement independent metrics and a verdict.",
    ))
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_evaluator(&mut evaluate)?;
    Ok(())
}
