use crate::protocol::PrepareRequest;
use crate::runtime::{
    streaming::{Decision, Progress, StreamingContext},
    worker_sdk::{Implementation, WorkerResult, serve_implementation},
};
use serde_json::json;

#[derive(Default)]
struct Halving {
    maximum_updates: u64,
}
impl Implementation for Halving {
    type Output = ();
    fn supports_one_shot(&self) -> bool {
        false
    }
    fn supports_streaming_profile(&self) -> bool {
        true
    }
    fn prepare(&mut self, request: &PrepareRequest) -> WorkerResult<()> {
        self.maximum_updates = request.implementation_parameters()["maximum_updates"]
            .as_u64()
            .unwrap_or(20);
        Ok(())
    }
    fn execute_stream(&mut self, stream: &mut StreamingContext<'_>) -> WorkerResult<()> {
        let mut x = 1.0;
        for index in 0..=self.maximum_updates {
            let progress = Progress {
                check_index: index,
                work: std::collections::BTreeMap::from([("updates".into(), index)]),
            };
            if matches!(
                stream.checkpoint(progress.clone(), || Ok(json!({"x":x})))?,
                Decision::Stop(_)
            ) {
                return Ok(());
            }
            if index == self.maximum_updates {
                stream.finish(progress, || Ok(json!({"x":x})))?;
                return Ok(());
            }
            x *= 0.5;
        }
        Ok(())
    }
    fn reset(&mut self) -> WorkerResult<()> {
        Ok(())
    }
}
pub fn serve() -> Result<(), crate::runtime::worker_sdk::ServeError> {
    serve_implementation(&mut Halving::default())
}
pub fn main() {
    serve().expect("native streaming worker");
}
