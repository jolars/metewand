#![allow(dead_code)]

mod bridge;
mod cluster;
mod dense;
mod dual;
mod flow;
mod fusedlasso;
mod path;
mod profile;
mod prox;
mod screening;
mod solver;
mod ssnal;
mod trajectory;

use metewand_protocol::PrepareRequest;
use metewand_runtime::{
    buffered::BufferedTrace,
    worker_sdk::{Implementation, WorkerError, WorkerResult, serve_implementation},
};
use serde_json::{Value, json};

struct Solver {
    method: solver::Method,
}

impl Solver {
    fn solve(&self, observed: bool) -> WorkerResult<Value> {
        let problem = solver::Problem::new(
            vec![
                vec![1.0, 0.0, 0.0],
                vec![0.0, 2.0, 0.0],
                vec![0.0, 0.0, 3.0],
            ],
            vec![-1.5, 1.05, 19.0 / 6.0],
            0.1,
            0.2,
        )
        .map_err(WorkerError::operation_failed)?;
        let options = solver::Options {
            method: self.method,
            lipschitz: 9.0,
            line_search: false,
            tolerance: 1e-12,
            max_iterations: 10000,
            check_every: 1,
            pg_every: 5,
            stopping: dual::Stopping::PgResidual,
        };
        let result = solver::solve(
            &problem,
            vec![0.0; 3],
            &options,
            &mut profile::Profile::<false>::default(),
            trajectory::Recorder::new(observed, None).map_err(WorkerError::operation_failed)?,
        )
        .map_err(WorkerError::operation_failed)?;
        Ok(json!({"coefficients": result.beta, "intercept": 0.0}))
    }
}

impl Implementation for Solver {
    type Output = Value;
    fn supports_buffered_profile(&self) -> bool {
        true
    }
    fn prepare(&mut self, _request: &PrepareRequest) -> WorkerResult<()> {
        Ok(())
    }
    fn execute(&mut self) -> WorkerResult<Value> {
        self.solve(false)
    }
    fn execute_buffered(&mut self, trace: &mut BufferedTrace) -> WorkerResult<()> {
        let collector = BufferedTrace::new(trace.profile.clone(), trace.directory.clone())?;
        *trace = bridge::collect(collector, || self.solve(true))?;
        Ok(())
    }
    fn reset(&mut self) -> WorkerResult<()> {
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let method = match std::env::args().nth(1).as_deref() {
        Some("pg") => solver::Method::Pg,
        Some("hybrid") => solver::Method::Hybrid,
        _ => return Err("expected pg or hybrid".into()),
    };
    serve_implementation(&mut Solver { method })?;
    Ok(())
}
