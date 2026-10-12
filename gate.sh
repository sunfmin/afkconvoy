#!/bin/sh
# The gate: what must pass before anything lands. This repo's own
# docs/agents/afkconvoy.md names it as `gate_command`.
set -e
cd "$(dirname "$0")"
# The whole gate runs on rustup's nightly, the one cargo-flux is built for,
# whatever the shell that started it has on its PATH: a Homebrew `cargo`
# ahead of rustup's breaks when Homebrew moves a library under it.
PATH="$HOME/.cargo/bin:$PATH"
RUSTUP_TOOLCHAIN=nightly-2026-09-25
export PATH RUSTUP_TOOLCHAIN
cargo fmt --all --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
# A test of the CLI waits on the processes it starts, and cargo runs one test
# program at a time: nextest runs them side by side. Without it the gate
# checks the same and takes about three times as long.
if cargo nextest --version >/dev/null 2>&1; then
    cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level fail --show-progress none
else
    echo "gate: cargo-nextest is not installed (brew install cargo-nextest): the tests run one program at a time" >&2
    cargo test --workspace --quiet
fi
# Every function of the core crate is checked: none is taken on trust.
if grep -rnE '#!?\[(flux_rs::)?(trusted|trusted_impl|ignore|opaque)' crates/core/src; then
    echo "gate: the core crate must not exempt anything from Flux" >&2
    exit 1
fi
./verify.sh
