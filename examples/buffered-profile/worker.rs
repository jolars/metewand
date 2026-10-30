//! A native trajectory with no host-language callback crossings.
use crate::protocol::PrepareRequest;
use crate::runtime::{
    buffered::{BufferedTrace, Decision, Progress},
    worker_sdk::{Implementation, WorkerResult, serve_implementation},
};
use serde_json::json;

#[derive(Default)]
struct Halving {
    maximum_updates: u64,
    local_bias: f64,
}
impl Implementation for Halving {
    type Output = ();
    fn supports_one_shot(&self) -> bool {
        false
    }
    fn supports_buffered_profile(&self) -> bool {
        true
    }
    fn prepare(&mut self, request: &PrepareRequest) -> WorkerResult<()> {
        self.maximum_updates = request.implementation_parameters()["maximum_updates"]
            .as_u64()
            .unwrap_or(20);
        self.local_bias = request.implementation_parameters()["local_bias"]
            .as_f64()
            .unwrap_or(0.0);
        Ok(())
    }
    fn execute_buffered(&mut self, trace: &mut BufferedTrace) -> WorkerResult<()> {
        let mut x = 1.0_f64;
        let mut index = 0;
        loop {
            let progress = Progress {
                check_index: index,
                work: std::collections::BTreeMap::from([("updates".into(), index)]),
            };
            let decision = trace.record(
                progress.clone(),
                json!({"gap":(x*x+self.local_bias).max(0.0)}),
                || Ok(json!({"x":x})),
            )?;
            if matches!(decision, Decision::Stop(_)) || index >= self.maximum_updates {
                trace.finish(
                    progress,
                    || Ok(json!({"gap":(x*x+self.local_bias).max(0.0)})),
                    || Ok(json!({"x":x})),
                )?;
                return Ok(());
            }
            x *= 0.5;
            index += 1;
        }
    }
    fn reset(&mut self) -> WorkerResult<()> {
        Ok(())
    }
}

pub fn serve() -> Result<(), crate::runtime::worker_sdk::ServeError> {
    serve_implementation(&mut Halving::default())
}
pub fn main() {
    serve().expect("native buffered worker");
}
