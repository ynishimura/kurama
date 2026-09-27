#!/usr/bin/env bash
# Remove the databases `up.sh` started. The TLS material stays: it is cheap to
# keep and the next `up.sh` reuses it.
set -euo pipefail
for name in kurama-pg17 kurama-pg18 kurama-mysql84; do
  docker rm -f "$name" >/dev/null 2>&1 || true
done
echo "databases are down"
