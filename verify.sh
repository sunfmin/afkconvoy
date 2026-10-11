#!/bin/sh
# Checks the core crate's refinement types with Flux (docs/adr/0007).
# Needs `cargo-flux`, `fixpoint` and `z3` installed.
#
#   ./verify.sh                 check the whole core crate
#   ./verify.sh free_slots      check the functions whose path contains a name
set -e
cd "$(dirname "$0")/crates/core"
# cargo-flux re-invokes `cargo +<its nightly>`, which only rustup's cargo
# understands, and rustup needs some toolchain to start from.
PATH="$HOME/.cargo/bin:$PATH"
: "${RUSTUP_TOOLCHAIN:=$(rustup toolchain list | head -1 | cut -d' ' -f1)}"
export PATH RUSTUP_TOOLCHAIN
case "${1:-}" in
"" | -* | check) ;;
*) set -- check --only-check="def:$1" ;;
esac
exec cargo flux "$@"
