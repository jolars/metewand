# 4. Add scientific validation

A successful process does not establish a correct or sufficiently accurate
answer. Add explicit rules and independently justified reference cases to the
problem from [part 1](index.md).

## Declare the rules

Keep the existing result, metrics, and evaluator declarations. Add this
`semantics` mapping under `problems.least_squares`:

```yaml
semantics:
  objective: "sum((X beta - y)^2) / (2 n)"
  preprocessing: {centering: false, scaling: false, intercept: false}
  validity: {beta: finite_vector_matching_feature_count}
  one_shot_completion: {gradient_inf: 1.0e-8}
```

This states the objective, preprocessing choices, validity requirement, and
accuracy target. The evaluator reads these rules through `context.semantics`.
Metewand does not infer formulas or implement the scientific meaning of a
rule's name; your evaluator must enforce it.

The evaluator from [part 2](functions.md#evaluate-the-answers-independently)
already checks coefficient dimensions, computes metrics, and compares the
gradient norm with the declared target. No solver changes are needed.

## Understand the verdicts

| Verdict | Meaning in this benchmark |
| --- | --- |
| `invalid` | The coefficient vector cannot represent a valid answer. |
| `reached` | The gradient infinity norm is at most `1e-8`. |
| `not_reached` | The coefficients are valid, but the gradient exceeds the target. |
| `not_required` | No completion target was declared. |

When validity or completion rules are declared, return an explicit
`Evaluation(metrics, verdict)`. Plain metrics cannot establish that the rules
were checked. A validity-only problem can return `not_required` for a valid
result when no completion rule applies.

A valid answer that misses the target retains its metrics and observation,
while its attempt fails with `completion_target_not_reached`. An unusable
answer needs no invented metric values, which is why the metric fields are
optional in this example.

## Check the evaluator against analytic cases

Add `reference-cases` beside `semantics`:

```yaml
reference-cases:
  - dataset: {x: [[1, 0], [0, 1]], y: [1, 2]}
    result: {beta: [1, 2]}
    expected-metrics: {objective: 0, gradient_inf: 0}
  - dataset: {x: [[1, 0], [0, 1]], y: [1, 2]}
    result: {beta: [0, 0]}
    expected-metrics: {objective: 1.25, gradient_inf: 1}
    expected-verdict: not_reached
  - dataset: {x: [[1, 0], [0, 1]], y: [1, 2]}
    result: {beta: [0]}
    expected-metrics: {}
    expected-verdict: invalid
```

For the identity design matrix, `beta = [1, 2]` fits exactly. At `beta = [0, 0]`,
the objective is `(1 + 4) / 4 = 1.25`, and the gradient is `[-0.5, -1]`.
A vector with only one coefficient has the wrong dimension. The omitted verdict
on the exact solution defaults to `reached` when a completion target exists.

These cases check the evaluator's formulas and verdicts. They do not run the
solvers or prove the solvers correct. Choose references from analytic answers,
independent calculations, or other trusted scientific evidence.

## Run the checks

```sh
metewand check
metewand check --workers
metewand run --output-dir validated-run
metewand status --output-dir validated-run
```

Static checking validates declarations and reference shapes. Worker checking
imports the evaluator and checks the references against its actual outputs.

If you kept the iteration-limit case from [part 3](parameters.md), expect
completion misses to remain visible. The run continues through the remaining
selection and exits with code `5` if any attempt fails. Inspect the recorded
evidence before deciding whether to change the methods, limits, or target.

Next, [control execution and analyze the results](running.md).
