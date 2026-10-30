"""Version-1 acquisition bridge using only Datamonger's public Python API."""

import hashlib
import json
import re
import sys
from importlib.metadata import version
from pathlib import Path

from datamonger import Registry, data_info, fetch_artifact, fetch_data


def main(request):
    if request["version"] != 1 or request["selector"]["version"] != 1:
        raise ValueError("unsupported Datamonger adapter version")
    selector = request["selector"]
    registry = Registry(**selector["registry"])
    dataset = selector["dataset"]
    kwargs = dict(
        source=dataset["source"],
        version=dataset["version"],
        registry=registry,
        cache_dir=request["cache_dir"],
        offline=request["offline"],
    )
    info = data_info(dataset["name"], **kwargs)
    expected_id = f"{dataset['source']}:{dataset['name']}@{dataset['version']}"
    if (
        info.dataset_id != expected_id
        or info.version != dataset["version"]
        or info.registry_release != registry.release
        or info.registry_index_sha256 != registry.index_sha256
    ):
        raise ValueError("Datamonger metadata differs from the pinned selector")
    if sum(artifact["size"] for artifact in info.artifacts) > request["max_bytes"]:
        raise ValueError("Datamonger artifacts exceed the byte budget")
    output = Path(request["output_dir"]) / "artifacts"
    output.mkdir()
    inventory = []
    for artifact in info.artifacts:
        name = artifact["name"]
        if re.fullmatch(r"[a-z0-9][a-z0-9._-]*", name) is None:
            raise ValueError("unsupported Datamonger artifact name")
        source = fetch_artifact(dataset["name"], artifact=name, **kwargs)
        # Public fetches return provider-owned paths. Copy and verify them
        # without moving, linking, or changing permissions on the cache object.
        digest = hashlib.sha256()
        size = 0
        with source.open("rb") as reader, (output / name).open("xb") as writer:
            while chunk := reader.read(65536):
                size += len(chunk)
                if size > artifact["size"]:
                    raise ValueError("Datamonger export exceeds the pinned size")
                digest.update(chunk)
                writer.write(chunk)
        if size != artifact["size"] or digest.hexdigest() != artifact["sha256"]:
            raise ValueError("Datamonger export differs from the artifact pin")
        inventory.append(
            dict(
                name=name,
                path=f"artifacts/{name}",
                size_bytes=size,
                sha256=artifact["sha256"],
                format=artifact["format"],
                compression=artifact["compression"],
            )
        )
    canonical_form = canonical_digest = None
    if selector["verification"] == "decoded":
        result = fetch_data(
            dataset["name"], verify_decoded=True, return_info=True, **kwargs
        )
        verified = result.info
        if (
            verified.verification != "decoded"
            or verified.dataset_id != expected_id
            or verified.registry_release != registry.release
            or verified.registry_index_sha256 != registry.index_sha256
        ):
            raise ValueError("Datamonger did not verify the selected decoded values")
        digests = {artifact["name"]: artifact["sha256"] for artifact in info.artifacts}
        inputs = set(info.representation["inputs"].values())
        if set(verified.artifact_digests) != inputs or any(
            digests[name] != digest
            for name, digest in verified.artifact_digests.items()
        ):
            raise ValueError("Datamonger decoded a different artifact selection")
        canonical_form, canonical_digest = (
            verified.canonical_form,
            verified.canonical_digest,
        )
    receipt = dict(
        version=1,
        dataset_id=info.dataset_id,
        dataset_version=info.version,
        registry=selector["registry"],
        representation=info.representation,
        artifacts=sorted(inventory, key=lambda artifact: artifact["path"]),
        verification=selector["verification"],
        canonical_form=canonical_form,
        canonical_digest=canonical_digest,
        client=dict(name="datamonger-python", version=version("datamonger")),
    )
    Path(request["receipt_path"]).write_text(
        json.dumps(receipt, sort_keys=True, separators=(",", ":"), allow_nan=False),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main(json.loads(sys.argv[1]))
