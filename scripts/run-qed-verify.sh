#!/usr/bin/env bash
# Run QED verification tests.
# Automatically skips prover-dependent tests when qed-prover is unavailable.

set -euo pipefail

echo "Running QED verification tests..."

if command -v qed-prover &>/dev/null; then
    echo "QED prover found: $(which qed-prover)"
    echo "Running all tests including prover-dependent ones..."
    cargo test -p metamorphosis-qed -- --include-ignored
else
    echo "QED prover not found — skipping prover-dependent tests."
    echo "Install with: ./scripts/install-qed-prover.sh"
    cargo test -p metamorphosis-qed
fi
