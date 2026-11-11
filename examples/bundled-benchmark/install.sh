#!/bin/sh
# Install bindings globally so isolated workers can import them.
python3 -m pip install --no-cache-dir -r requirements.txt
