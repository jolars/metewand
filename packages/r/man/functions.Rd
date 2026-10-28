\name{serve_function}
\alias{serve_function}
\title{Serve an Ordinary Benchmark Function}
\description{Load a manifest-selected function inside a worker process and adapt
it to the CLI-owned dataset, implementation, or evaluator lifecycle.}
\usage{
serve_function(role = NULL, file = NULL, name = NULL)
}
\arguments{
  \item{role}{One of dataset_materializer, implementation, or problem_evaluator.
  When NULL, read role, file, and name from the three trailing command arguments.}
  \item{file}{The selected source file, sourced during the worker handshake.}
  \item{name}{The name of the function defined in that file.}
}
\details{Dataset functions take context; implementation functions take data and
context; evaluator functions take data, result, and context. Context is a list
containing parameters, problem_parameters, seed, and semantics. Seeds are
available for generation and solving; scientific rules are available for
evaluation. Unavailable objects are empty JSON objects and unavailable seeds
are NULL. Callbacks return named lists or explicit output values. The CLI writes data.json,
dataset inventories, and standard result and metric envelopes.

Without validity or completion rules, plain metric objects receive the verdict
not_required. Declared rules require an explicit evaluation object. Completion
verdicts require completion rules. Use lifecycle workers for profiles, custom
artifacts, and separate preparation.}
\value{Returns invisibly after shutdown or a rejected handshake.}
