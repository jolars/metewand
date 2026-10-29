# Metewand TODO

Remaining work is grouped by area. Prioritize useful benchmarks in Basin and
clustlasso-cd.

[DESIGN.md](DESIGN.md) defines the general model.
[CONTRIBUTING.md](CONTRIBUTING.md) records implementation contracts and
verification, and [AGENTS.md](AGENTS.md) gives repository instructions.

## Adopter integrations

Keep scientific definitions, adapters, and retained benchmark outputs in the
adopter repositories. Keep small regression cases in Metewand that can run
without the adopter dependencies.

- [ ] Exercise a Basin trace through buffered profiles with selected result
  snapshots. Reuse `../basin/crates/competitor-bench`, including
  `src/bin/trace.rs`.
- [ ] Exercise a clustlasso-cd native trajectory through buffered profiles with
  selected independently evaluated coefficient snapshots. Reuse
  `~/research/clustlasso-cd/experiments/real_data.py` and the existing
  independent evaluator.
- [ ] Complete full intercept-strategy and SLOPE study ports if still useful.

Use the [buffered profile example](examples/buffered-profile) as a starting
point. Exercise metric disagreement, snapshot mutation, bounds, partial
publication, and failures in the adopter workflows. Preserve retry isolation and
reject unreturned buffers.

## Applicability

- [ ] Keep excluded candidates visible in plans, status, explanation, and
  exports. Exercise a real representation or dimension restriction from an
  adopter and distinguish it from a failed attempt.

## Profiles

- [ ] Add adaptive scientific-budget schedules.
- [ ] Add checkpoint-state restoration.

## Language packages and environments

- [ ] Add the Julia worker package.
- [ ] Add `renv` resolution as adopters need it, including local dependencies
  and offline verification.
- [ ] Add Julia `Pkg` and Conda environment resolution.

See [Reproducibility model](DESIGN.md#reproducibility-model) for environment
guarantees and the existing Nix and uv backends.

## Execution and isolation

- [ ] Add digest-pinned OCI environments and Docker and Podman execution on
  Linux.
- [ ] Enforce requested resource, network, read-only input, and process-tree
  controls, with explicit failures when required enforcement is unavailable.
- [ ] Complete archival machine provenance and accounting, and qualify a
  mixed-language benchmark under those stronger guarantees.
- [ ] Add stronger isolation backends as needed.
- [ ] Add SLURM and remote execution.

Publication-archive claims require the enforced controls described in the
[reproducibility model](DESIGN.md#reproducibility-model).

## Results and analysis

- [ ] Extend export formats and provenance packaging when an analysis needs more
  than the existing JSONL and CSV tables and lineage receipt.
- [ ] Add provenance-tracked analysis artifacts.
- [ ] Add a read-only diagnostic viewer.

## Tooling

- [ ] Add an optional MCP adapter over the CLI.
