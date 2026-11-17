# Time the solver

The [default timer](running.md#choose-what-to-time) includes preparing the
computation and writing its result. If you want to compare time spent inside the
solver callback, you can choose that measurement explicitly.

Use this recipe when the extra work around a solver would obscure the comparison
you want to make. Keep the default when preparing and returning an answer is
part of the work you want to measure.

## Allow untimed preparation {#choose-what-to-time}

First decide what the method may do before its timer starts. Loading shared
input data might be allowed, while setup specific to a method might belong in
the measured work. State that choice as part of the problem's definition so that
every method follows the same rule.

In the least-squares example, add this field under `problems.least_squares` to
permit both the default scope and an execution-only scope:

```yaml
allowed-timing-scopes: [prepare_and_execute, execute_only]
```

## Select the solver timer {#name-an-execution-policy}

Add this policy at the top level of the manifest:

```yaml
execution-policies:
  solver:
    timing-scope: execute_only
    primary-time: implementation_time
    timeout: 5 s
```

Then add `execution-policy: solver` to the experiment. Metewand will use the
elapsed time around the worker's `execute` callback as the comparison's primary
time. The supplied Python, R, and Rust adapters report this measurement.

## Keep the comparison fair {#time-the-solver-callback}

Work you do inside the callback still counts. For example, converting an array
to a list inside your solver function is part of its measured time. Metewand's
later conversion and writing of the returned result happen outside this clock.

Use [separate preparation and solving
callbacks](extending.md#separate-preparation-and-solving) when you need more
control over that boundary. Keep method-specific setup inside the timer when
your comparison should charge for it.

Metewand also records the outer wall time. Scientific timeouts still use that
wall time, including result writing. Choosing the callback timer does not give
the worker extra time to finish.

Return to [running the comparison](running.md) to collect and inspect the
measurements.
