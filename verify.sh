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
#   ./verify.sh install         install, under the home directory, each of the
#                               three that is not the pinned one, then check
#                               as --toolchain does: that gate need's install
set -e
cd "$(dirname "$0")/crates/core"
# cargo-flux re-invokes `cargo +<its nightly>`, which only rustup's cargo
# understands, and rustup needs some toolchain to start from. What `install`
# puts in ~/.local/bin comes first, so the tools checked are the ones it
# installs.
PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
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
flux_is() { cargo flux --version 2>/dev/null | cut -d' ' -f2; }
fixpoint_is() { fixpoint --version 2>/dev/null | sed -n 's/^fixpoint //p'; }
z3_is() { z3 --version 2>/dev/null | sed -n 's/^Z3 version \([^ ]*\).*/\1/p'; }
flux_pinned() {
    flux_is=$(flux_is) || true
    case "$flux_is" in
    "") return 1 ;;
    *) case "$flux_rev" in "$flux_is"*) ;; *) return 1 ;; esac ;;
    esac
}
fixpoint_pinned() {
    fixpoint_is=$(fixpoint_is) || true
    [ "$fixpoint_is" = "$fixpoint_pin" ]
}
z3_pinned() {
    z3_is=$(z3_is) || true
    [ "$z3_is" = "$z3_pin" ]
}
# `install` installs what is not pinned, reading the pins above, and writes
# only under the home directory, with no sudo (docs/adr/0053): z3 from its
# release, cargo-flux and fixpoint built from their sources at the pinned
# commits, in clones kept in the user's cache. A tool already at its pin is
# left alone, so a run that finds all three pinned does nothing.
if [ "${1:-}" = install ]; then
    src="${XDG_CACHE_HOME:-$HOME/.cache}/src"
    mkdir -p "$src" "$HOME/.local/bin"
    work=$(mktemp -d "${TMPDIR:-/tmp}/verify-install.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) z3_platform=arm64-osx stack_platform=osx-aarch64 ;;
    Darwin-x86_64) z3_platform=x64-osx stack_platform=osx-x86_64 ;;
    Linux-aarch64) z3_platform=arm64-glibc stack_platform=linux-aarch64 ;;
    Linux-x86_64) z3_platform=x64-glibc stack_platform=linux-x86_64 ;;
    *)
        echo "verify: install knows no release of z3 or stack for $(uname -s) $(uname -m)" >&2
        exit 1
        ;;
    esac
    # Clones $1 into $2, once, and checks out the commit $3, fetching it only
    # when the clone has not got it.
    checkout() {
        [ -d "$2/.git" ] || git clone --quiet "$1" "$2"
        git -C "$2" checkout --quiet --force "$3" 2>/dev/null || {
            git -C "$2" fetch --quiet origin
            git -C "$2" checkout --quiet --force "$3"
        }
    }
    took() { echo "verify: installed $1 in $(($(date +%s) - began))s" >&2; }
    if ! z3_pinned; then
        began=$(date +%s)
        # The release names its macOS and glibc builds by the system version
        # they were built on, which the pin does not say: the release says it.
        z3_zip=$(curl -fsSL "https://api.github.com/repos/Z3Prover/z3/releases/tags/z3-$z3_pin" |
            sed -n "s|.*\"browser_download_url\": *\"\([^\"]*/z3-$z3_pin-$z3_platform-[^\"/]*\.zip\)\".*|\1|p" | head -1)
        [ -n "$z3_zip" ] || {
            echo "verify: the z3 release z3-$z3_pin has no $z3_platform zip" >&2
            exit 1
        }
        curl -fsSL -o "$work/z3.zip" "$z3_zip"
        unzip -q "$work/z3.zip" -d "$work/z3"
        rm -rf "$HOME/.local/z3-$z3_pin"
        mv "$work/z3/"z3-"$z3_pin"-* "$HOME/.local/z3-$z3_pin"
        ln -sf "$HOME/.local/z3-$z3_pin/bin/z3" "$HOME/.local/bin/z3"
        took "z3 $z3_pin"
    fi
    if ! flux_pinned; then
        began=$(date +%s)
        checkout https://github.com/flux-rs/flux "$src/flux" "$flux_rev"
        # Flux names its own nightly in its rust-toolchain.toml, which the
        # RUSTUP_TOOLCHAIN set above would override.
        (cd "$src/flux" && env -u RUSTUP_TOOLCHAIN cargo xtask install)
        took "cargo-flux $flux_rev"
    fi
    if ! fixpoint_pinned; then
        began=$(date +%s)
        # No release is built at the pinned commit: stack builds it, with the
        # GHC liquid-fixpoint's stack.yaml names, which stack fetches into
        # ~/.stack. The stack used is the one pinned beside fixpoint's, kept
        # apart from any other stack on the PATH.
        stack_pin=$(sed -n 's/^stack = "\(.*\)"$/\1/p' Cargo.toml)
        [ -n "$stack_pin" ] || {
            echo "verify: crates/core/Cargo.toml must pin the stack that builds fixpoint, and it is not read from it" >&2
            exit 1
        }
        stack="$HOME/.local/stack-$stack_pin/stack"
        if [ ! -x "$stack" ]; then
            curl -fsSL -o "$work/stack.tar.gz" \
                "https://github.com/commercialhaskell/stack/releases/download/v$stack_pin/stack-$stack_pin-$stack_platform.tar.gz"
            mkdir -p "$work/stack" "$HOME/.local/stack-$stack_pin"
            tar -xzf "$work/stack.tar.gz" -C "$work/stack"
            mv "$work/stack/stack-$stack_pin-$stack_platform/stack" "$stack"
        fi
        fixpoint_commit=$(echo "$fixpoint_pin" | sed -n 's/.*(\([0-9a-f]*\))$/\1/p')
        checkout https://github.com/ucsd-progsys/liquid-fixpoint "$src/liquid-fixpoint" "$fixpoint_commit"
        (cd "$src/liquid-fixpoint" && "$stack" install --local-bin-path "$HOME/.local/bin")
        took "fixpoint $fixpoint_pin"
    fi
    set -- --toolchain
fi
pinned=yes
if ! flux_pinned; then
    pinned=
    cat >&2 <<EOF
verify: cargo-flux is built from Flux ${flux_is:-(nothing reported)}, and crates/core/Cargo.toml pins $flux_rev. Install the pinned one, with ./verify.sh install, or:
    git clone https://github.com/flux-rs/flux && cd flux && git checkout $flux_rev && cargo xtask install
EOF
fi
if ! fixpoint_pinned; then
    pinned=
    cat >&2 <<EOF
verify: fixpoint is ${fixpoint_is:-(nothing reported)}, and crates/core/Cargo.toml pins $fixpoint_pin. Install the pinned one, built at the commit in the brackets, with ./verify.sh install, or:
    https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install
EOF
fi
if ! z3_pinned; then
    pinned=
    cat >&2 <<EOF
verify: z3 is ${z3_is:-(nothing reported)}, and crates/core/Cargo.toml pins $z3_pin. Install the pinned one, with ./verify.sh install, or:
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
