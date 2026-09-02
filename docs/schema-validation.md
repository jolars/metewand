# JSON Schema validation and parameter defaults

Metewand version 1 validates repository contracts with JSON Schema 2020-12. A
schema document must declare the dialect explicitly:

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

The catalog contains the complete set of resources available to the validator.
An external reference succeeds only if its target was supplied explicitly;
validation never retrieves a schema from the filesystem or network. Manifest
and lockfile processing will later determine which verified repository files
are admitted to this catalog.

## Parameter resolution

Definitions may provide a literal `parameter_defaults` object. Metewand resolves
parameters in this order:

1. Begin with `parameter_defaults`, or an empty object when it is absent.
2. Merge supplied parameters recursively when both values at a key are objects.
3. Otherwise, replace the default with the supplied value.
4. Validate the complete resolved object against its parameter schema.

Arrays are literal values and are never traversed or expanded during this
merge. Scalars and explicit `null` values also replace their defaults. Both the
defaults and supplied parameter roots must be objects.

The JSON Schema `default` keyword remains an annotation. It never adds a value
to the resolved parameters. Only an explicit manifest `parameter_defaults`
object contributes an omitted value. The resolved object remains in Metewand's
[canonical JSON domain](canonical-json.md) and is the sole parameter object used
by later identity and transport operations.

Versioned conformance fixtures for local references and parameter schemas live
under [`fixtures/schema-validation/v1`](../fixtures/schema-validation/v1/).
