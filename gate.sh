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
# The manual is the help topics as one page, and is never edited: a topic
# changed and the page not written again fails here, with the command to run.
cargo run --quiet -p afkconvoy --example manual -- --check
# A test of the CLI waits on the processes it starts, and cargo runs one test
# program at a time: nextest runs them side by side. Without it the gate
# checks the same and takes about three times as long.
if cargo nextest --version >/dev/null 2>&1; then
    cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level fail --show-progress none
else
    echo "gate: cargo-nextest is not installed (brew install cargo-nextest): the tests run one program at a time" >&2
    cargo test --workspace --quiet
fi
# Every function of the core crate is checked: none is taken on trust. Flux
# says so itself, in the summary that ends its run on a crate, so an exemption
# counts however it is written. Flux summarises a dependency too: the core's
# summary is the one before cargo's last artifact message, the core being the
# crate checked and so the last one finished.
flux_log=$(mktemp)
trap 'rm -f "$flux_log"' EXIT
./verify.sh --message-format=json-render-diagnostics >"$flux_log" 2>&1 && flux_status=0 || flux_status=$?
core_summary=$(awk '
    /^\{/ { if (/"reason":"compiler-artifact"/) { core = last; last = "" }; next }
    { print > "/dev/stderr" }
    /^summary\./ { last = $0 }
    END { print core }' "$flux_log")
[ "$flux_status" -eq 0 ] || exit "$flux_status"
case "$core_summary" in
*"; 0 trusted; 0 ignored."*) ;;
*)
    echo "gate: the core crate must not exempt anything from Flux, and Flux reports: ${core_summary:-no summary of the core crate}" >&2
    exit 1
    ;;
esac
