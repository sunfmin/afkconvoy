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
# Flux takes about a minute on one core and builds under a directory of its
# own, so it checks the core crate while everything below runs: what it came
# to is read at the end. It times every function it checks (`-Ftimings`),
# and keeps the file it writes of them out of the tree.
flux_log=$(mktemp)
FLUXFLAGS="-Ftimings -Flog-dir=$PWD/target/flux-log"
export FLUXFLAGS
./verify.sh --message-format=json-render-diagnostics >"$flux_log" 2>&1 &
flux_pid=$!
trap 'kill "$flux_pid" 2>/dev/null || :; rm -f "$flux_log"' EXIT
cargo clippy --workspace --all-targets --quiet -- -D warnings
# The manual is the help topics as one page, and is never edited: a topic
# changed and the page not written again fails here, with the command to run.
cargo run --quiet -p afkconvoy --example manual -- --check
# A test of the CLI waits on the processes it starts, and cargo runs one test
# program at a time: nextest runs them side by side. Without it the gate
# checks the same and takes about three times as long. nextest also holds one
# test to the limit .config/nextest.toml gives: a test past it is ended, and
# fails by name as TIMEOUT. `cargo test` has no such limit.
if cargo nextest --version >/dev/null 2>&1; then
    cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level fail --show-progress none || {
        tests_status=$?
        echo "gate: a test listed above as TIMEOUT ran too long: it was ended at the limit for one test, which .config/nextest.toml gives" >&2
        exit "$tests_status"
    }
else
    echo "gate: cargo-nextest is not installed (brew install cargo-nextest): the tests run one program at a time, with no limit on one test" >&2
    cargo test --workspace --quiet
fi
# Every function of the core crate is checked: none is taken on trust. Flux
# says so itself, in the summary that ends its run on a crate, so an exemption
# counts however it is written. Flux summarises a dependency too: the core's
# summary is the one before cargo's last artifact message, the core being the
# crate checked and so the last one finished.
#
# Flux reports the slowest functions of each crate too. The gate holds the
# reports back and ends with the core's, green or red, so the end of its
# output names the functions that are slow to check. The core's is the last
# report: a crate that fails is the last one checked.
wait "$flux_pid" && flux_status=0 || flux_status=$?
core_summary=$(awk '
    /^\{/ { if (/"reason":"compiler-artifact"/) { core = last; last = "" }; next }
    /Timing Report/ { timings = ""; in_timings = 1 }
    in_timings && !/^(───|Total running time:|Functions checked:|Min:|Max:|Mean:|Std\. Dev\.:|Top 5 Functions|• )/ { in_timings = 0 }
    in_timings { timings = timings $0 "\n"; next }
    { print > "/dev/stderr" }
    /^summary\./ { last = $0 }
    END {
        print core
        printf "%s", timings > "'"$flux_log.timings"'"
    }' "$flux_log")
flux_timings=$(cat "$flux_log.timings")
rm -f "$flux_log.timings"
slowest() {
    echo "gate: what Flux took longest to check in the core crate:" >&2
    echo "${flux_timings:-Flux reported no timings.}" >&2
}
[ "$flux_status" -eq 0 ] || {
    slowest
    exit "$flux_status"
}
case "$core_summary" in
*"; 0 trusted; 0 ignored."*) ;;
*)
    echo "gate: the core crate must not exempt anything from Flux, and Flux reports: ${core_summary:-no summary of the core crate}" >&2
    slowest
    exit 1
    ;;
esac
slowest
