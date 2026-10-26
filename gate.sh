#!/bin/sh
# The gate: what must pass before anything lands. This repo's own
# docs/agents/afkconvoy.md names it as `gate_command`, run through
# `afkconvoy exclusive`: every landing turn uses the host's worker lanes.
# Run by hand, it is
# `afkconvoy exclusive -- ./gate.sh`.
set -e
cd "$(dirname "$0")"
# The whole gate runs on rustup's nightly, the one cargo-flux is built for,
# whatever the shell that started it has on its PATH: a Homebrew `cargo`
# ahead of rustup's breaks when Homebrew moves a library under it.
PATH="$HOME/.cargo/bin:$PATH"
RUSTUP_TOOLCHAIN=nightly-2026-09-25
export PATH RUSTUP_TOOLCHAIN
# Each step's seconds, said as the gate ends, green or red: the fleet times
# only the whole gate (docs/adr/0024), and which step grew is for its log to
# say. A red gate says the steps it finished.
began=$(date +%s)
took=
step() {
    now=$(date +%s)
    took="$took $1 $((now - began))s"
    began=$now
}
steps() {
    echo "gate: the seconds each step took:${took:- none finished}" >&2
}
trap steps EXIT
cargo fmt --all --check
step fmt
# Every function of the core crate is checked: none is taken on trust. Flux
# says so itself, in the summary that ends its run on a crate, so an exemption
# counts however it is written. Flux summarises a dependency too: the core's
# summary is the one before cargo's last artifact message, the core being the
# crate checked and so the last one finished.
#
# Flux reports the slowest functions of each crate too. The gate holds the
# reports back and says the core's once Flux has ended, green or red, so its
# output names the functions that are slow to check. The core's is the last
# report: a crate that fails is the last one checked.
#
# Flux runs first, alone, before anything else is built: run beside the tests
# it shares the cores with every one of them, and how long it took says more of
# them than of it. It takes about a minute on one core, builds under a
# directory of its own, times every function it checks (`-Ftimings`) and keeps
# the file it writes of them out of the tree. A solver query of one function
# past twenty seconds is stopped, and is an error naming that function
# (`-Ffixpoint-timeout`): no query holds the gate up longer. Flux checks on
# past an error (`-Fcatch-bugs`), so a run names every function that is slow,
# and still ends with its timings. verify.sh sets them for every run.
flux_log=$(mktemp)
trap 'rm -f "$flux_log"; steps' EXIT
./verify.sh --message-format=json-render-diagnostics >"$flux_log" 2>&1 && flux_status=0 || flux_status=$?
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
step flux
cargo clippy --workspace --all-targets --quiet -- -D warnings
step clippy
# The manual is the help topics as one page, and is never edited: a topic
# changed and the page not written again fails here, with the command to run.
cargo run --quiet -p afkconvoy --example manual -- --check
step manual
# A test of the CLI waits on the processes it starts, and cargo runs one test
# program at a time: nextest runs them side by side. Without it the gate
# checks the same and takes about three times as long. nextest also holds one
# test to the limit .config/nextest.toml gives: a test past it is ended, and
# fails by name as TIMEOUT, and names each test past the time it gives as
# SLOW once the run ends. `cargo test` has neither.
#
cargo nextest --version >/dev/null 2>&1 && nextest=yes || nextest=
# The tests are built first, under the TMPDIR the gate was given: a build
# that is the first to reach sccache after its server went idle starts one,
# which binds its socket under its TMPDIR, and under the one below no socket
# fits (#439). The run below builds nothing more.
if [ -n "$nextest" ]; then
    cargo nextest run --workspace --cargo-quiet --no-run
else
    cargo test --workspace --quiet --no-run
fi
step build
# The tests run with a TMPDIR deeper than a worker's ever was, whoever runs
# the gate: inside this checkout, in the directory that ignores itself, and
# deeper than a socket's path may be long. A test that binds a socket by its
# whole path there, finds this checkout above its own directories, or says a
# path whole fails in every gate, and not only in a worker's.
TMPDIR="$PWD/.afkconvoy-tmp/the-tests-of-the-gate-run-in-a-directory-as-deep-as-a-worker-s"
mkdir -p "$TMPDIR"
printf '*\n' >"$PWD/.afkconvoy-tmp/.gitignore"
export TMPDIR
if [ -n "$nextest" ]; then
    cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level slow --show-progress none || {
        tests_status=$?
        echo "gate: a test listed above as TIMEOUT ran too long: it was ended at the limit for one test, which .config/nextest.toml gives" >&2
        exit "$tests_status"
    }
else
    echo "gate: cargo-nextest is not installed (brew install cargo-nextest): the tests run one program at a time, with no limit on one test" >&2
    cargo test --workspace --quiet
fi
step tests
