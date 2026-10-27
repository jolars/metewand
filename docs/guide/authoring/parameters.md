# 3. Vary parameters and add repetitions

Keep the functions from [part 2](functions.md). You can expand the comparison
by editing only the manifest.

## Put settings with their owner

The dataset's `n_samples`, `n_features`, and `noise` change its generated data.
Each method's `iterations` changes how it solves the problem. A problem
parameter would change the requested computation, such as a regularization
coefficient. Timing and process controls belong in execution policies.

Types and bounds declare valid values. A `default` supplies a missing value.
A `grid` selects values to benchmark without narrowing their valid domain:

```yaml
n_samples: {type: integer, minimum: 1, grid: [100, 1000]}
```

Replace the dataset's original `n_samples` declaration with this one to try
both sizes. A field can have a default or a grid, but not both. Fields are
required unless `optional: true`; unknown parameter names are rejected.

## Select experiment values

To try two iteration limits for each method, replace the experiment with:

```yaml
experiments:
  - name: comparison
    problem: least_squares
    datasets: [simulated]
    implementations: [gradient_descent, coordinate_descent]
    implementation-parameters:
      gradient_descent:
        iterations: {grid: [20, 200]}
      coordinate_descent:
        iterations: {grid: [20, 200]}
    measurement-repetitions: 3
    seed: 7
```

With two dataset sizes, two methods, and two iteration limits per method, this
produces eight candidates. Three measurement repetitions produce 24 measured
attempt slots before retries. `metewand plan` shows the expansion and the
effective parameter values before any code runs.

Repetition counts default to one. Measurement repetitions repeat a configured
computation. `implementation-repetitions` selects separate implementation seed
blocks. Retrying a failed attempt is a separate operation and does not add a
scientific repetition.

Experiment overrides use `dataset-parameters.<dataset>`, `problem-parameters`,
and `implementation-parameters.<implementation>`. A scalar supplies one value,
such as `iterations: 20`. For a literal array or object, use `value`, such as
`weights: {value: [1, 2]}`, rather than an unwrapped array.

## Use named cases for coupled choices

A grid forms combinations. Named cases keep choices together. For a small
comparison between the normal and deliberately limited methods, replace the
previous experiment with:

```yaml
experiments:
  - name: comparison
    problem: least_squares
    datasets: [simulated]
    implementations: [gradient_descent, coordinate_descent]
    measurement-repetitions: 3
    seed: 7
    cases:
      - name: normal
      - name: iteration-limit
        implementation-parameters:
          gradient_descent: {iterations: 1}
          coordinate_descent: {iterations: 1}
```

The normal case uses each method's default of 200 iterations. The second case
uses one iteration for each. Both inherit the dataset grid if you kept it.
After adding an accuracy target, this will show the difference between a valid
answer and one that reaches the requested accuracy.

Selection follows this order: declaration grids supply initial choices,
experiment axes replace matching choices, and named cases replace matching
experiment axes. Expanded values merge with literal defaults and must still
satisfy their types and bounds.

## Inspect before scaling up

```sh
metewand check
metewand plan
metewand plan --output json
```

Check the candidate count and resolved settings before running a large matrix.
Identical candidates are rejected rather than silently counted twice. See the
[parameter-matrix example](https://github.com/jolars/metewand/tree/main/examples/parameter-matrix)
for dataset, problem, and implementation overrides together.

Next, [add scientific validation](validation.md).
