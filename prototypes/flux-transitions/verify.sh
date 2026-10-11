#!/bin/sh
# Verifies the refinement types with Flux (https://flux-rs.github.io/flux).
# Needs `cargo-flux`, `fixpoint` and `z3` installed.
#
#   ./verify.sh                 verify the whole crate
#   ./verify.sh remove_all      verify one function
set -e
cd "$(dirname "$0")"
# cargo-flux re-invokes `cargo +<its nightly>`, which only rustup's cargo
# understands, and rustup needs some toolchain to start from.
PATH="$HOME/.cargo/bin:$PATH"
: "${RUSTUP_TOOLCHAIN:=$(rustup toolchain list | head -1 | cut -d' ' -f1)}"
export PATH RUSTUP_TOOLCHAIN
# `./verify.sh <name>` checks only the functions whose path contains <name>,
# through one of Flux's include patterns; other arguments go to cargo flux.
case "${1:-}" in
"" | -* | check) ;;
*) set -- check --only-check="def:$1" ;;
esac
exec cargo flux "$@"
