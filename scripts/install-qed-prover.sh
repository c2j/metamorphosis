#!/usr/bin/env bash
# Build and install QED prover from source.
#
# Usage: ./scripts/install-qed-prover.sh
#
# Prerequisites:
#   - Rust nightly toolchain (rustup toolchain install nightly)
#   - libclang and z3 headers (brew install z3 on macOS, apt install libz3-dev z3 on Linux)
#
# Environment variables:
#   QED_PROVER_DIR      - install directory (default: /usr/local/bin)
#   QED_PROVER_REPO     - git repo URL (default: https://github.com/qed-solver/prover)
#
# Also installs cvc5 if not already on PATH (downloads static binary).

set -euo pipefail

INSTALL_DIR="${QED_PROVER_DIR:-/usr/local/bin}"
REPO="${QED_PROVER_REPO:-https://github.com/qed-solver/prover}"
BUILD_DIR="$(mktemp -d)"

echo "=== Installing QED prover dependencies ==="

OS="$(uname -s)"
ARCH="$(uname -m)"

# Install Z3 if not present
if ! command -v z3 &>/dev/null; then
    echo "Installing Z3..."
    case "${OS}" in
        Darwin) brew install z3 ;;
        Linux)
            sudo apt-get update -qq && sudo apt-get install -y -qq z3 libz3-dev
            ;;
        *) echo "Unsupported OS: ${OS}" >&2; exit 1 ;;
    esac
fi
echo "Z3: $(z3 --version 2>&1 | head -1)"

# Install CVC5 if not present
if ! command -v cvc5 &>/dev/null; then
    echo "Installing CVC5..."
    case "${OS}-${ARCH}" in
        Darwin-arm64)
            CVC5_URL="https://github.com/cvc5/cvc5/releases/download/cvc5-1.3.4/cvc5-macOS-arm64-static.zip"
            ;;
        Darwin-x86_64)
            CVC5_URL="https://github.com/cvc5/cvc5/releases/download/cvc5-1.3.4/cvc5-macOS-x86_64-static.zip"
            ;;
        Linux-arm64)
            CVC5_URL="https://github.com/cvc5/cvc5/releases/download/cvc5-1.3.4/cvc5-Linux-arm64-static.zip"
            ;;
        Linux-x86_64)
            CVC5_URL="https://github.com/cvc5/cvc5/releases/download/cvc5-1.3.4/cvc5-Linux-static.zip"
            ;;
        *) echo "No pre-built CVC5 for ${OS}-${ARCH}, skipping" ; CVC5_URL="" ;;
    esac
    if [ -n "${CVC5_URL}" ]; then
        curl -fsSL "${CVC5_URL}" -o /tmp/cvc5-static.zip
        unzip -o /tmp/cvc5-static.zip -d /tmp/cvc5-install
        CVC5_BIN="$(find /tmp/cvc5-install -name 'cvc5' -type f | head -1)"
        if [ -n "${CVC5_BIN}" ]; then
            cp "${CVC5_BIN}" "${INSTALL_DIR}/cvc5"
            chmod +x "${INSTALL_DIR}/cvc5"
            echo "CVC5 installed: ${INSTALL_DIR}/cvc5"
        fi
        rm -rf /tmp/cvc5-static.zip /tmp/cvc5-install
    fi
else
    echo "CVC5: $(cvc5 --version 2>&1 | head -1)"
fi

# Ensure nightly toolchain is available
if ! rustup toolchain list | grep -q nightly; then
    echo "Installing Rust nightly toolchain..."
    rustup toolchain install nightly
fi

# Build qed-prover from source
echo "=== Building qed-prover from source ==="
git clone --depth 1 "${REPO}" "${BUILD_DIR}/prover"
cd "${BUILD_DIR}/prover"
cargo +nightly build --release

BINARY="${BUILD_DIR}/prover/target/release/qed-prover"
if [ -f "${BINARY}" ]; then
    cp "${BINARY}" "${INSTALL_DIR}/qed-prover"
    chmod +x "${INSTALL_DIR}/qed-prover"
    echo "QED prover installed: ${INSTALL_DIR}/qed-prover"
else
    echo "ERROR: Build failed — qed-prover binary not found" >&2
    exit 1
fi

# Cleanup
rm -rf "${BUILD_DIR}"
echo "=== Done ==="
