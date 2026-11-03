use super::*;

/// Shared problem construction uses the standard materialization output envelope.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareProblemRequest {
    pub id: RequestId,
    method: MaterializeMethod,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    pub dataset_dir: String,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    pub problem_contract_id: String,
    #[serde(deserialize_with = "deserialize_object")]
    pub problem_parameters: Value,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    pub output_dir: String,
}

impl private::Sealed for PrepareProblemRequest {}
impl WireMessage for PrepareProblemRequest {}

impl PrepareProblemRequest {
    pub fn new(
        id: RequestId,
        dataset_dir: String,
        problem_contract_id: String,
        problem_parameters: Value,
        output_dir: String,
    ) -> Self {
        Self {
            id,
            method: MaterializeMethod::Materialize,
            dataset_dir,
            problem_contract_id,
            problem_parameters,
            output_dir,
        }
    }
}
