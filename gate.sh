#!/bin/sh
# The gate: what must pass before anything lands. This repo's own
# docs/agents/afkconvoy.md names it as `gate_command`.
set -e
cd "$(dirname "$0")"
cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
cargo test --workspace --quiet
# Every function of the core crate is checked: none is taken on trust.
if grep -rnE '#!?\[(flux_rs::)?(trusted|trusted_impl|ignore|opaque)' crates/core/src; then
    echo "gate: the core crate must not exempt anything from Flux" >&2
    exit 1
fi
./verify.sh
