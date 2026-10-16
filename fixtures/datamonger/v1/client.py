#!@PYTHON@
"""Test double for the public Datamonger API used by the process bridge."""

import hashlib
import importlib.metadata
import json
from pathlib import Path
import sys
import time
from types import ModuleType, SimpleNamespace

root = Path(@ROOT@)
behavior = @BEHAVIOR@
if behavior == "timeout":
    time.sleep(60)

client = ModuleType("datamonger")
client.Registry = lambda **kwargs: SimpleNamespace(**kwargs)
importlib.metadata.version = lambda name: "test-client-v1"
payload = b"value\n1\n2\n3\n"
digest = hashlib.sha256(payload).hexdigest()
representation = {
    "decoder": "delimited-text",
    "decoder_version": 1,
    "inputs": {"data": "data"},
    "options": {"delimiter": ",", "header": True},
    "expect": {"verification": [{"canonical_form": 1, "algorithm": "sha256", "digest": "69abd67c3fa8b7ea2992b7a5091adf6626066bd2cb1e3e2e92222de8cc0fedfa"}]},
}


def data_info(name, *, source, version, registry, cache_dir, offline):
    assert (name, source, version) == ("numbers", "test", "1")
    assert registry.release == "metewand-test-1"
    assert registry.index_sha256 == "11" * 32
    return SimpleNamespace(
        dataset_id="test:numbers@2" if behavior == "dataset_mismatch" else "test:numbers@1",
        version=version,
        registry_release=registry.release,
        registry_index_sha256="00" * 32 if behavior == "registry_mismatch" else registry.index_sha256,
        artifacts=({"name": "data", "format": "csv", "compression": "none", "size": len(payload), "sha256": digest},),
        representation=representation,
    )


def fetch_artifact(name, *, source, version, artifact, registry, cache_dir, offline):
    assert artifact == "data"
    cache = Path(cache_dir)
    cache.mkdir(exist_ok=True)
    path = cache / "data"
    path.write_bytes(b"bad" if behavior == "artifact_mismatch" else payload)
    return path


def fetch_data(name, *, source, version, registry, cache_dir, offline, verify_decoded, return_info):
    assert verify_decoded is True and return_info is True
    if behavior == "decoded_failure":
        raise ValueError("decoded digest mismatch")
    return SimpleNamespace(info=SimpleNamespace(
        dataset_id="test:numbers@1",
        registry_release=registry.release,
        registry_index_sha256=registry.index_sha256,
        artifact_digests={"data": digest},
        verification="artifact" if behavior == "verification_downgrade" else "decoded",
        canonical_form=1,
        canonical_digest="69abd67c3fa8b7ea2992b7a5091adf6626066bd2cb1e3e2e92222de8cc0fedfa",
    ))


client.data_info = data_info
client.fetch_artifact = fetch_artifact
client.fetch_data = fetch_data
sys.modules["datamonger"] = client
assert sys.argv[1:3] == ["-I", "-c"]
script = sys.argv[3]
sys.argv = ["-c", sys.argv[4]]
assert sys.argv[1] == json.dumps(json.loads(sys.argv[1]), sort_keys=True, separators=(",", ":"))
exec(compile(script, "datamonger_bridge.py", "exec"))
if behavior == "extra_export":
    request = json.loads(sys.argv[1])
    (Path(request["output_dir"]) / "extra").write_bytes(b"undeclared")
if behavior.startswith("receipt_"):
    request = json.loads(sys.argv[1])
    path = Path(request["receipt_path"])
    receipt = json.loads(path.read_bytes())
    if behavior == "receipt_wrong_selector":
        receipt["registry"]["index_sha256"] = "00" * 32
    elif behavior == "receipt_wrong_digest":
        receipt["canonical_digest"] = "00" * 32
    elif behavior == "receipt_wrong_path":
        receipt["artifacts"][0]["path"] = "elsewhere"
    elif behavior == "receipt_future_version":
        receipt["version"] = 2
    elif behavior == "receipt_missing_decoded_digest":
        receipt["canonical_digest"] = None
    elif behavior == "receipt_duplicate_artifact":
        receipt["artifacts"].append(receipt["artifacts"][0])
    elif behavior == "receipt_oversized":
        receipt["client"]["version"] = "x" * (1024 * 1024)
    path.write_text(json.dumps(receipt))
