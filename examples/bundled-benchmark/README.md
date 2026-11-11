# Bundle a whole benchmark

This example puts the runner and all workers in one image. It requires Docker
with Linux container support. Generate the context from the repository root:

```sh
metewand bundle --manifest examples/bundled-benchmark/metewand.yaml \
  --destination /tmp/metewand-context \
  --base-image python@sha256:af8a5aa14780025dbfbcfbc81bd8a91ebd87711dd5b0f48ed71090949dd7cb1d \
  --runner-version 0.5.0 --recipe install.sh --include requirements.txt
cd /tmp/metewand-context
docker build -t metewand-benchmark .
mkdir -p output
docker run --rm --network none --user "$(id -u):$(id -g)" \
  --mount type=bind,src="$PWD/output",dst=/output metewand-benchmark
```

Results remain in `output/run` after the container exits. Repeat the run command
to resume automatically. To inspect stored results:

```sh
docker run --rm --network none --entrypoint /usr/local/bin/metewand \
  --mount type=bind,src="$PWD/output",dst=/output metewand-benchmark \
  status --output-dir /output/run
```

Generation reads local files only. Docker builds fetch the pinned runner release
and install the explicit dependency recipe. Worker bindings have independent
versions; this example pins Python binding version 0.4.0. A supplied base image
that already contains dependencies needs no recipe. Additional recipe inputs
must be selected with `--include`.

`bundle.json` inventories context content, excluding itself. Keep the generated
context and the built image digest with retained results. Bundling preserves
logical identities and the existing trusted execution guarantees; it does not
establish archival isolation or prove a complete dependency closure.
