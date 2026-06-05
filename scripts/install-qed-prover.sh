#!/usr/bin/env bash
# Install QED prover binary from GitHub releases.
#
# Usage: ./scripts/install-qed-prover.sh
#
# Environment variables:
#   QED_PROVER_VERSION  - version tag (default: latest)
#   QED_PROVER_DIR      - install directory (default: /usr/local/bin)
#
# If the binary is unavailable for the current platform, exits silently
# (QED tests will be skipped).

set -euo pipefail

VERSION="${QED_PROVER_VERSION:-latest}"
INSTALL_DIR="${QED_PROVER_DIR:-/usr/local/bin}"
REPO="qed-solver/prover"

echo "Installing QED prover (${VERSION}) to ${INSTALL_DIR}..."

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "Unsupported architecture: ${ARCH}" >&2; exit 0 ;;
esac

if [ "${VERSION}" = "latest" ]; then
    URL="https://github.com/${REPO}/releases/latest/download/qed-prover-${OS}-${ARCH}"
else
    URL="https://github.com/${REPO}/releases/download/${VERSION}/qed-prover-${OS}-${ARCH}"
fi

BINARY="${INSTALL_DIR}/qed-prover"
mkdir -p "${INSTALL_DIR}"

if curl -fsSL "${URL}" -o "${BINARY}" 2>/dev/null; then
    chmod +x "${BINARY}"
    echo "QED prover installed: ${BINARY}"
else
    echo "Note: QED prover binary not available for ${OS}-${ARCH}."
    echo "      Build from source: git clone https://github.com/${REPO} && cd prover && cargo build --release"
    echo "      Continuing without prover — QED tests will be skipped."
fi
