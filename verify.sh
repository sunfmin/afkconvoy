#!/bin/sh
# Checks the core crate's refinement types with Flux (docs/adr/0007).
# Needs `cargo-flux`, `fixpoint` and `z3` installed, each the one
# crates/core/Cargo.toml pins: it refuses to check with any other.
#
#   ./verify.sh                 check the whole core crate
#   ./verify.sh free_slots      check the functions whose path contains a name
#   ./verify.sh --toolchain     check only that the three tools are the pinned
#                               ones, and exit: the gate need that
#                               docs/agents/afkconvoy.md declares
set -e
cd "$(dirname "$0")/crates/core"
# cargo-flux re-invokes `cargo +<its nightly>`, which only rustup's cargo
# understands, and rustup needs some toolchain to start from.
PATH="$HOME/.cargo/bin:$PATH"
: "${RUSTUP_TOOLCHAIN:=$(rustup toolchain list | head -1 | cut -d' ' -f1)}"
export PATH RUSTUP_TOOLCHAIN
# The checker is the pinned one, or nothing is checked: another build of it
# can pass or fail the same tree differently. Each tool is asked what it is,
# which is as far as this goes:
#   cargo-flux  reports the first seven digits of the Flux revision it was
#               built from, and they are compared to the pinned revision's.
#               Not checked: the rest of the revision, a change made to that
#               source before the build, and the driver and libraries in
#               ~/.flux, which report no revision and are installed with it.
#   fixpoint    reports its version and its whole commit: both are compared.
#   z3          reports its version, which is compared, and nothing of its
#               build.
flux_rev=$(sed -n 's/^flux-rs = .*rev = "\([0-9a-f]*\)".*/\1/p' Cargo.toml)
fixpoint_pin=$(sed -n 's/^fixpoint = "\(.*\)"$/\1/p' Cargo.toml)
z3_pin=$(sed -n 's/^z3 = "\(.*\)"$/\1/p' Cargo.toml)
if [ -z "$flux_rev" ] || [ -z "$fixpoint_pin" ] || [ -z "$z3_pin" ]; then
    echo "verify: crates/core/Cargo.toml must pin the Flux revision, fixpoint and z3, and one of them is not read from it" >&2
    exit 1
fi
flux_is=$(cargo flux --version 2>/dev/null | cut -d' ' -f2) || true
fixpoint_is=$(fixpoint --version 2>/dev/null | sed -n 's/^fixpoint //p') || true
z3_is=$(z3 --version 2>/dev/null | sed -n 's/^Z3 version \([^ ]*\).*/\1/p') || true
pinned=yes
case "$flux_is" in
"") pinned= ;;
*) case "$flux_rev" in "$flux_is"*) ;; *) pinned= ;; esac ;;
esac
if [ -z "$pinned" ]; then
    cat >&2 <<EOF
verify: cargo-flux is built from Flux ${flux_is:-(nothing reported)}, and crates/core/Cargo.toml pins $flux_rev. Install the pinned one:
    git clone https://github.com/flux-rs/flux && cd flux && git checkout $flux_rev && cargo xtask install
EOF
fi
if [ "$fixpoint_is" != "$fixpoint_pin" ]; then
    pinned=
    cat >&2 <<EOF
verify: fixpoint is ${fixpoint_is:-(nothing reported)}, and crates/core/Cargo.toml pins $fixpoint_pin. Install the pinned one, built at the commit in the brackets:
    https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install
EOF
fi
if [ "$z3_is" != "$z3_pin" ]; then
    pinned=
    cat >&2 <<EOF
verify: z3 is ${z3_is:-(nothing reported)}, and crates/core/Cargo.toml pins $z3_pin. Install the pinned one:
    https://github.com/Z3Prover/z3/releases/tag/z3-$z3_pin
EOF
fi
[ -n "$pinned" ] || exit 1
if [ "${1:-}" = --toolchain ]; then
    exit 0
fi
# Every run, the gate's or a hand's, stops a solver query of one function past
# sixty seconds as an error naming it, checks on past it so one run names
# every slow function, and times each, the file of timings kept out of the
# tree: a run is never held up by one query.
FLUXFLAGS="-Ftimings -Ffixpoint-timeout=60 -Fcatch-bugs -Flog-dir=$PWD/../../target/flux-log${FLUXFLAGS:+ $FLUXFLAGS}"
export FLUXFLAGS
case "${1:-}" in
"" | -* | check) ;;
*) set -- check --only-check="def:$1" ;;
esac
exec cargo flux "$@"
