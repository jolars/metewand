import io
from unittest.mock import patch

from metewand_adapter import serve_bindings
from metewand_adapter._canonical import canonical_bytes, parse_json
from metewand_adapter._worker import _serve

WORKER_ID = "mw1-resolved-worker-" + "a" * 64
METADATA = {"name": "metewand-python", "version": "42.0.0"}


def hello(role="implementation", **changes):
    return dict(
        dict(id="0", method="hello", protocols=[1], role=role, worker_id=WORKER_ID),
        **changes,
    )


def request(method, **fields):
    return dict(id="1", method=method, **fields)


def serve(callbacks, role, reader, writer):
    def transport(worker, role, **options):
        _serve(worker, role, reader, writer, **options)

    with patch("metewand_adapter._worker._serve_socket", side_effect=transport):
        serve_bindings(api_version=1, role=role, callbacks=callbacks, metadata=METADATA)


def exchange(callbacks, role, requests):
    source = io.BytesIO(b"".join(canonical_bytes(r) + b"\n" for r in requests))
    output = io.BytesIO()
    serve(callbacks, role, source, output)
    lines = output.getvalue().splitlines()
    for line in lines:
        assert canonical_bytes(parse_json(line)) == line
    return [parse_json(line) for line in lines]
