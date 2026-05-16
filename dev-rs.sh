#!/bin/bash
set -e

# Unlighthouse Rust Dev Script
# This script builds the UI, syncs assets, and runs the Rust core.

# 1. Build UI
echo "📦 Building Vue Client..."
pnpm -C packages/client build

# 2. Sync assets
echo "🚚 Syncing assets to .unlighthouse/client..."
mkdir -p .unlighthouse/client
cp -r packages/client/dist/* .unlighthouse/client/

# 3. Build Rust Core
echo "🦀 Building Rust Core..."
cargo build --manifest-path packages/core-rs/Cargo.toml

# 4. Run Rust Core
echo "🚀 Launching Unlighthouse Rust..."
# We run from the root so that .unlighthouse path resolves correctly by default
./packages/core-rs/target/debug/unlighthouse-rs --lighthouse-process-path ./packages/core-rs/lighthouse.mjs "$@"
