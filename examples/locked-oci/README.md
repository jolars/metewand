# Locked Docker workers

This example runs Python functions in a digest-pinned image on a local, rootful
Linux Docker Engine. Docker supplies the Python binding package; Metewand stages
its matching runtime adapter. The image also supplies all scientific dependencies.

From the repository root, build and push the image to a registry you control:

```sh
docker build -f examples/locked-oci/Dockerfile -t REGISTRY/metewand-example:bench .
docker push REGISTRY/metewand-example:bench
docker image inspect --format '{{json .RepoDigests}}' REGISTRY/metewand-example:bench
```

Replace the placeholder `image` in `metewand.yaml` with the resulting
`REGISTRY/metewand-example@sha256:...` reference. Keep retained benchmark outputs
outside this example directory. Then run:

```sh
metewand lock --manifest examples/locked-oci/metewand.yaml
metewand lock --manifest examples/locked-oci/metewand.yaml --check
metewand run --manifest examples/locked-oci/metewand.yaml --locked --output-dir /tmp/metewand-oci-run
```

Only `lock` pulls images. Checks and locked runs use the prepared Docker content
store and never fetch or build software. Verification trusts that store's image
metadata; it does not independently rehash image filesystem contents. The native
platform is selected without emulation. Remote engines, rootless engines, and
user namespace remapping are unsupported.

Workers use host networking for authenticated loopback protocol sockets. Metewand
mounts its private staging root at the same path, runs workers as the invoking
UID/GID, and overrides the image entrypoint. Programs and Python/R runtimes must
come from the image. Explicit pipe and stdio transports are unsupported. Native
workers using the repository helper also support sockets.

This backend runs trusted code. It retains input-integrity checks, but it does
not enforce general resource or filesystem policies. CPU and memory accounting
are unavailable, and the container umask is unknown. Container creation and
startup count toward `cold_end_to_end`. Normal failures and timeouts remove owned
containers; a hard orchestrator crash can leave containers behind. Find those
with `docker ps -a --filter label=org.metewand.worker=true` and remove their IDs
explicitly after inspecting them.
