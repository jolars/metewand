use basin::{Executor, NelderMead, SimplexProgress, State, StepOutcome, problems::Rosenbrock};
use competitor_bench::nm;
use crate::protocol::PrepareRequest;
use crate::runtime::{
    buffered::{BufferedTrace, Decision, Progress},
    worker_sdk::{Implementation, WorkerError, WorkerResult, serve_implementation},
};
use serde_json::{Value, json};

struct Solver {
    method: String,
}

impl Implementation for Solver {
    type Output = Value;

    fn supports_buffered_profile(&self) -> bool {
        self.method == "basin"
    }

    fn prepare(&mut self, _request: &PrepareRequest) -> WorkerResult<()> {
        Ok(())
    }

    fn execute(&mut self) -> WorkerResult<Value> {
        let result = match self.method.as_str() {
            "basin" => nm::solve_basin(2),
            "gomez" => nm::solve_gomez::<false>(2),
            _ => return Err(WorkerError::operation_failed("unknown method")),
        };
        Ok(json!({"x": result.point, "local_objective": result.cost}))
    }

    fn execute_buffered(&mut self, trace: &mut BufferedTrace) -> WorkerResult<()> {
        let mut stepper = Executor::new(
            Rosenbrock::<Vec<f64>>::default(),
            NelderMead::new(),
            SimplexProgress::from_simplex(nm::initial_simplex(2)),
        )
        .max_iter(nm::MAX_ITERS)
        .into_stepper()
        .map_err(|e| WorkerError::operation_failed(e.to_string()))?;
        for index in 0..=nm::MAX_ITERS {
            let outcome = stepper
                .step()
                .map_err(|e| WorkerError::operation_failed(e.to_string()))?;
            let state = stepper.state();
            let progress = Progress {
                check_index: index,
                work: std::collections::BTreeMap::from([("iterations".into(), state.iter())]),
            };
            let metric = json!({"objective": state.cost()});
            let snapshot = || Ok(json!({"x": state.param(), "local_objective": state.cost()}));
            if matches!(
                trace.record(progress.clone(), metric.clone(), snapshot)?,
                Decision::Stop(_)
            ) || matches!(outcome, StepOutcome::Stopped(_))
            {
                trace.finish(progress, || Ok(metric), snapshot)?;
                return Ok(());
            }
        }
        Err(WorkerError::operation_failed(
            "native iteration bound exceeded",
        ))
    }

    fn reset(&mut self) -> WorkerResult<()> {
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_implementation(&mut Solver {
        method: std::env::args().nth(1).ok_or("expected method")?,
    })?;
    Ok(())
}
