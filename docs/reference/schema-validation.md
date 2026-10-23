# JSON Schema validation and parameter defaults

Metewand version 1 validates repository contracts with JSON Schema 2020-12. A
standalone schema document must declare the dialect explicitly:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}
```

The core validation API accepts an in-memory catalog whose keys are normalized,
UTF-8, repository-relative paths. It validates every document against the
Draft 2020-12 meta-schema before compiling it. Relative references use the
referencing document's repository path as their base, so a schema at
`schemas/result.json` may refer to `common.json#/$defs/file` to select a
definition from `schemas/common.json`.

The repository loader admits declared schemas and their reachable local
dependencies. The catalog contains the complete set of resources available to
the validator; validation never retrieves a schema from the filesystem or
network. Absolute URI references work only when an admitted schema declares the
matching `$id`. Relative `$id` values establish a new reference base, and
`$dynamicRef` dependencies follow the same offline rules as `$ref`.

## Inline declarations

Every authored schema field accepts a repository-relative JSON file path, a
schema mapping, or a boolean schema. This includes dataset output and parameter
schemas, implementation parameter schemas, and every schema in a problem
contract. Inline schemas inherit Draft 2020-12, so these declarations validate
the same parameter domain:

```yaml
parameter-schema: schemas/iterations.json
```

```yaml
parameter-schema:
  type: object
  properties:
    iterations: { type: integer, minimum: 1 }
  required: [iterations]
  additionalProperties: false
```

The referenced JSON file must contain the same mapping plus its explicit
`$schema` declaration. Inline mappings may also declare `$schema`, but an
unsupported dialect fails validation. Both forms pass through the same
meta-schema validation, offline compilation, and instance validation.

An inline schema's relative references start in the directory containing its
owning manifest or external problem contract, unless `$id` changes the base.
For example, an inline schema in `problems/sum.yaml` can use
`$ref: common.json#/$defs/parameters` to load `problems/common.json`.
Local fragments such as `#/$defs/value` refer to the individual schema, rather
than the enclosing manifest. References cannot escape the repository, including
through symlinks. Values inside `const`, `default`, `enum`, and examples remain
literal data and do not introduce dependencies.

YAML anchors can share one inline dataset schema between `output-schema` and a
contract's `dataset-schemas`. A dataset declaration also matches an accepted
inline or file schema with identical normalized contents and reference bases.
Metewand does not infer conversions or prove equivalence between different
schema expressions. Two explicit file declarations retain their path-based
matching behavior.

Inline mappings normalize by adding the inherited `$schema` and applying the
[canonical JSON rules](canonical-json.md). Boolean `true` normalizes to that
dialect-only mapping; `false` adds `allOf: [false]`. The catalog uses a virtual
sibling path named `.metewand-inline-<sha256>.schema.json`, where the digest
hashes the normalized canonical schema bytes. It creates no schema files.
Schema identities include normalized contents and all transitively referenced
documents. Changing a dependency changes dependent identities.

Existing file schema identities retain their repository paths and canonical
representation. Moving a schema from a file into a declaration can therefore
change its identity. An inline contract and a file contract produce the same
contract identity when their effective declarations and reference bases match.

See [problem contracts](problem-contracts.md) and the runnable
[Python SDK example](https://github.com/jolars/metewand/tree/main/examples/python-sdk)
for a benchmark with one manifest, worker code, and scientific fixtures.

## Parameter resolution

Definitions may provide a literal `parameter-defaults` object. Metewand resolves
parameters in this order:

1. Begin with `parameter-defaults`, or an empty object when it is absent.
2. Merge supplied parameters recursively when both values at a key are objects.
3. Otherwise, replace the default with the supplied value.
4. Validate the complete resolved object against its parameter schema.

Arrays are literal values and are never traversed or expanded during this
merge. Scalars and explicit `null` values also replace their defaults. Both the
defaults and supplied parameter roots must be objects.

The JSON Schema `default` keyword remains an annotation. It never adds a value
to the resolved parameters. Only an explicit manifest `parameter-defaults`
object contributes an omitted value. The resolved object remains in Metewand's
[canonical JSON domain](canonical-json.md) and is the sole parameter object used
by later identity and transport operations.

Versioned conformance fixtures for local references and parameter schemas live
under [`fixtures/schema-validation/v1`](https://github.com/jolars/metewand/tree/main/fixtures/schema-validation/v1/).
Metewand-owned record contracts and their colocated conformance cases are
documented separately under [Public schemas](public-schemas.md).
