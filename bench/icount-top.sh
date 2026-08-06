#!/usr/bin/env bash
# Show the allocator-relevant lines of the last callgrind profile.
set -uo pipefail
f="${1:-/tmp/cg.ra.out}"
callgrind_annotate --threshold=70 "$f" 2>/dev/null \
  | grep -E 'rusty_alloc|tls_get|mimalloc|PROGRAM TOTALS|malloc|free' \
  | head -14
