#!/usr/bin/env bash
# Install the real-world validation set (§ "10 OSS programs" sweep).
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq \
  redis-server redis-tools sqlite3 jq xz-utils zstd lua5.4 python3 \
  git imagemagick nginx bc perl >/dev/null 2>&1
echo "INSTALL-DONE"
for b in redis-server redis-benchmark sqlite3 jq xz zstd lua5.4 python3 git convert nginx perl; do
  if command -v "$b" >/dev/null 2>&1; then echo "OK      $b"; else echo "MISSING $b"; fi
done
