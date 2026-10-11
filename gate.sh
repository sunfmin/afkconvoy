#!/bin/sh
# The gate: what must pass before anything lands. This repo's own
# docs/agents/afkconvoy.md names it as `gate_command`.
set -e
cd "$(dirname "$0")"
cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
cargo test --workspace --quiet
./verify.sh
