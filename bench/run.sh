#!/bin/sh
# DESIGN section 2.4 / 7.2 refer to bench/run.sh; the runner itself is Python.
exec python3 "$(dirname "$0")/run.py" "$@"
