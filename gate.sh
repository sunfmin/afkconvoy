#!/bin/sh
# The gate: what must pass before anything lands. This repo's own
# docs/agents/afkconvoy.md names it as `gate_command`, run through
# `afkconvoy exclusive`: every landing turn uses the host's worker lanes.
# Run by hand, it is
# `afkconvoy exclusive -- ./gate.sh`.
#
# It is two gates, run one after the other (docs/adr/0047):
#   ./gate.sh fast   the fast gate, which a worker passes before it opens or
#                    updates its PR: fmt, Flux, clippy, the manual, the build
#                    of every test, and the tests that start no process
#   ./gate.sh slow   the slow gate: every other test, and nothing the fast
#                    gate checks
#   ./gate.sh        both: the whole gate, which a landing turn runs
# The same docs/agents/afkconvoy.md declares each one's limit and budget.
set -e
cd "$(dirname "$0")"
case "${1:-}" in
"") fast=yes slow=yes ;;
fast) fast=yes slow= ;;
slow) fast= slow=yes ;;
*)
    echo "gate: run it as ./gate.sh, ./gate.sh fast or ./gate.sh slow, not ./gate.sh $*" >&2
    exit 2
    ;;
esac
# The line between the two gates is drawn per test: .config/slow-tests names
# each test that took 2 seconds or more on the host the line was drawn on, and
# runs only in the slow gate; every other test runs only in the fast gate. A
# new test is fast until it is named there. The names become one nextest
# filter, and its negation is the fast gate's, so no test runs in both or in
# neither.
slow_tests=$(awk '
    /^[[:space:]]*(#|$)/ { next }
    {
        binary = $1
        sub(/^[[:space:]]*[^[:space:]]+[[:space:]]+/, "")
        sub(/[[:space:]]+$/, "")
        printf "%s(binary_id(=%s) & test(=%s))", (n++ ? " | " : ""), binary, $0
    }
    END { if (!n) printf "none()" }' .config/slow-tests)
PATH="$HOME/.cargo/bin:$PATH"
RUSTUP_TOOLCHAIN=nightly-2026-09-25
export PATH RUSTUP_TOOLCHAIN
# Each step's seconds, said as the gate ends, green or red: the fleet times
# only the whole gate (docs/adr/0024), and which step grew is for its log to
# say. A red gate says the steps it finished.
began=$(date +%s)
echo "gate: ${1:-fast and slow} gate started $(date -u '+%Y-%m-%dT%H:%M:%SZ') on $(hostname), commit $(git rev-parse HEAD)" >&2
echo "gate: host $(uname -sm); $(uptime)" >&2
if [ -d target ]; then
    echo "gate: target/ present; this alone does not establish warm artifacts" >&2
else
    echo "gate: target/ absent; shared compiler and dependency caches may still be warm" >&2
fi
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
# The tests run with a TMPDIR deeper than a worker's ever was, whoever runs
# the gate: inside this checkout, in the directory that ignores itself, and
# deeper than a socket's path may be long. A test that binds a socket by its
# whole path there, finds this checkout above its own directories, or says a
# path whole fails in every gate, and not only in a worker's. It is set only
# where the tests run, below: the builds keep the TMPDIR the gate was given.
tests_tmpdir="$PWD/.afkconvoy-tmp/the-tests-of-the-gate-run-in-a-directory-as-deep-as-a-worker-s"
# A test of the CLI waits on the processes it starts, and cargo runs one test
# program at a time: nextest runs them side by side. Without it the gate
# checks the same and takes about three times as long. nextest also holds one
# test to the limit .config/nextest.toml gives: a test past it is ended, and
# fails by name as TIMEOUT, and names each test past the time it gives as
# SLOW once the run ends. `cargo test` has neither.
#
cargo nextest --version >/dev/null 2>&1 && nextest=yes || nextest=
[ -n "$nextest" ] ||
    echo "gate: cargo-nextest is not installed (brew install cargo-nextest): the tests run one program at a time, with no limit on one test" >&2
# Runs the tests nextest's filter `$2` picks under its profile `$1`, with
# the deep TMPDIR. Without nextest no test is told from another, and only the
# whole gate runs its tests, all of them, with cargo test.
run_tests() {
    mkdir -p "$tests_tmpdir"
    printf '*\n' >"$PWD/.afkconvoy-tmp/.gitignore"
    if [ -z "$nextest" ]; then
        TMPDIR="$tests_tmpdir" cargo test --workspace --quiet
        return
    fi
    TMPDIR="$tests_tmpdir" cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level slow --show-progress none --no-tests=pass --profile "$1" -E "$2" || {
        tests_status=$?
        case "$1" in
        fast) echo "gate: a test listed above as TIMEOUT is a slow test: it ran past 6 seconds, three times the fast gate's line of 2, and was ended. Name it in .config/slow-tests, as its binary and its name (\`afkconvoy::cli switch::its_name\`), and it runs in the slow gate" >&2 ;;
        *) echo "gate: a test listed above as TIMEOUT ran too long: it was ended at the limit for one test, which .config/nextest.toml gives" >&2 ;;
        esac
        exit "$tests_status"
    }
}
if [ -z "$nextest" ] && [ -n "${1:-}" ]; then
    echo "gate: the fast gate and the slow gate are told apart by nextest's filter, and cargo-nextest is not installed: run ./gate.sh, the whole gate" >&2
    exit 2
fi
if [ -n "$fast" ]; then
# Ignoring temp directories does not stop a forced add. Refuse tracked paths
# at every depth, including temp files and workers' private harness homes.
tracked_tmp=$(git ls-files -- ':(glob)**/.afkconvoy-tmp/**')
if [ -n "$tracked_tmp" ]; then
    echo "gate: no path under .afkconvoy-tmp may be tracked; remove these paths from git:" >&2
    printf '%s\n' "$tracked_tmp" >&2
    exit 1
fi
step tracked-temp
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
# past sixty seconds is stopped, and is an error naming that function
# (`-Ffixpoint-timeout`): no query holds the gate up longer. Twenty was too
# few beside other gates: stage_record::history, the slowest, takes 3s on an
# idle host and passed 20s under the load of 2026-10-29. Flux checks on
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
# The tests are built first, under the TMPDIR the gate was given: a build
# that is the first to reach sccache after its server went idle starts one,
# which binds its socket under its TMPDIR, and under the one below no socket
# fits (#439). The runs below build nothing more.
if [ -n "$nextest" ]; then
    cargo nextest run --workspace --cargo-quiet --no-run
else
    cargo test --workspace --quiet --no-run
fi
step build
# Without nextest, every test runs in the whole gate's slow part, below.
[ -z "$nextest" ] || {
    run_tests fast "not ($slow_tests)"
    step fast-tests
}
fi
if [ -n "$slow" ]; then
run_tests slow "$slow_tests"
step slow-tests
fi
