"""Compatibility entrypoint for function launches resolved by the CLI."""

import sys
from . import _serve

if len(sys.argv) != 4:
    raise SystemExit("Usage: python -m metewand ROLE FILE FUNCTION")
_serve(sys.argv[1], {"function": (sys.argv[2], sys.argv[3])})
