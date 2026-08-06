#!/usr/bin/env bash
# Corpus subset, all arms, interleaved arm order per benchmark (ra,mi,sys then
# sys,mi,ra) — the Tier-A M7 gate. Full-suite weekly job runs everything.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
for b in cfrac espresso larson malloc-large; do
  for arm in ra mi sys; do bash "$here/run-ra.sh" "$arm" "$b" 1; done
  for arm in sys mi ra; do bash "$here/run-ra.sh" "$arm" "$b" 1; done
done
