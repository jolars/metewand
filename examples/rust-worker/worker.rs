//! A native implementation and independent evaluator with no host-language process.

use std::path::Path;

use crate::protocol::{EvaluateRequest, EvaluationVerdict, PrepareRequest};
use crate::runtime::worker_sdk::{
    Evaluation, Implementation, WorkerResult, read_json, serve_evaluator, serve_implementation,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Sum {
    values: Vec<i64>,
    offset: i64,
    bias: i64,
}

impl Implementation for Sum {
    type Output = Value;

    fn prepare(&mut self, request: &PrepareRequest) -> WorkerResult<()> {
        self.values = serde_json::from_value(
            read_json(&Path::new(request.dataset_dir()).join("data.json"))?["values"].clone(),
        )?;
        self.offset = serde_json::from_value(request.problem_parameters()["offset"].clone())?;
        self.bias = serde_json::from_value(request.implementation_parameters()["bias"].clone())?;
        Ok(())
    }

    fn execute(&mut self) -> WorkerResult<Value> {
        Ok(json!({"answer": self.values.iter().sum::<i64>() + self.offset + self.bias}))
    }

    fn statistics(&mut self, _output: &Value) -> WorkerResult<Value> {
        Ok(json!({"input_count": self.values.len()}))
    }

    fn reset(&mut self) -> WorkerResult<()> {
        self.values.clear();
        Ok(())
    }
}

fn evaluate(request: &EvaluateRequest) -> WorkerResult<Evaluation> {
    let dataset = read_json(&Path::new(request.dataset_dir()).join("data.json"))?;
    let values: Vec<i64> = serde_json::from_value(dataset["values"].clone())?;
    let offset: i64 = serde_json::from_value(request.problem_parameters()["offset"].clone())?;
    let result = read_json(&Path::new(request.result_dir()).join("result.json"))?;
    let answer: i64 = serde_json::from_value(result["data"]["answer"].clone())?;
    let error = (answer - (values.iter().sum::<i64>() + offset)).abs();
    let rules = request
        .problem_semantics()
        .expect("package negotiated scientific rules");
    let target: i64 =
        serde_json::from_value(rules["one_shot_completion"]["absolute_error"].clone())?;
    Ok(Evaluation::new(
        json!({"absolute_error": error}),
        if error <= target {
            EvaluationVerdict::Reached
        } else {
            EvaluationVerdict::NotReached
        },
    ))
}

pub fn serve(role: &str) -> Result<(), Box<dyn std::error::Error>> {
    match role {
        "implementation" => serve_implementation(&mut Sum::default())?,
        "problem_evaluator" => serve_evaluator(&mut evaluate)?,
        _ => return Err("expected implementation or problem_evaluator".into()),
    }
    Ok(())
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve(&std::env::args().nth(1).ok_or("expected a worker role")?)
}
