"""Function worker entrypoint used by Metewand's local launcher."""

import sys

from ._functions import serve_function

if len(sys.argv) != 4:
    raise SystemExit("Usage: python -m metewand ROLE FILE FUNCTION")
serve_function(*sys.argv[1:])
