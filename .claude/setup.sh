#!/bin/bash
# Project environment setup script
# Ensures all required tools are installed

set -e

# Check and install Rust/Cargo via rustup
if ! command -v cargo &>/dev/null; then
    echo "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.85
    source "$HOME/.cargo/env"
fi

# Ensure rustfmt and clippy are installed
rustup component add rustfmt clippy 2>/dev/null || true

# Check and install just
if ! command -v just &>/dev/null; then
    echo "Installing just..."
    cargo install just
fi

# Check for rsync (warn if missing, don't fail)
if ! command -v rsync &>/dev/null; then
    echo "Warning: rsync not found. Install with: apt-get install rsync (or brew install rsync)"
fi

# Check for ssh (warn if missing, don't fail)
if ! command -v ssh &>/dev/null; then
    echo "Warning: ssh not found. Install with: apt-get install openssh-client (or brew install openssh)"
fi

echo "Environment ready."
