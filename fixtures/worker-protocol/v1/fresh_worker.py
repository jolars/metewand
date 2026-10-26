"""Raw fresh-sequence implementation fixture without an SDK dependency."""

import json
import os
from pathlib import Path
import sys

mode = sys.argv[1] if len(sys.argv) > 1 else "valid"
reader = os.fdopen(os.dup(int(os.environ["METEWAND_PROTOCOL_READ_FD"])), "r")
writer = os.fdopen(os.dup(int(os.environ["METEWAND_PROTOCOL_WRITE_FD"])), "w")
prepared = False
for line in reader:
    request = json.loads(line)
    method = request["method"]
    response = {"id": request["id"], "ok": True}
    if method == "hello":
        response.update(protocol=1, worker_id=request["worker_id"], sdk=None,
                        capabilities=["fresh_sequence"])
    elif method == "prepare":
        prepared = True
    elif method == "execute":
        assert prepared
        prepared = False
        control = request["observation_control"]
        lower, upper = 1.0, 2.0
        for _ in range(control["iterations"]):
            midpoint = (lower + upper) / 2
            if midpoint * midpoint < 2:
                lower = midpoint
            else:
                upper = midpoint
        result = Path(request["result_dir"]) / "result.json"
        result.write_text(json.dumps({"schema_version": 1, "data": {"x": (lower + upper) / 2}, "files": []}))
        echo = {"iterations": float(control["iterations"])} if mode == "float_echo" else control
        if mode == "mismatch":
            echo = {"iterations": 99}
        response.update(result={"manifest": "result.json"}, implementation_time_ns=None,
                        statistics={}, observation_control=echo)
    elif method == "reset":
        prepared = False
    elif method != "shutdown":
        raise ValueError(method)
    writer.write(json.dumps(response, sort_keys=True, separators=(",", ":")) + "\n")
    writer.flush()
    if method == "shutdown":
        break
