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
# The tests leave no process behind: one that outlives them, stopped or
# running, holds the worktree it ran in, and settling waits on it
# (sunfmin/control#508). Once the tests end, a process whose working directory
# or executable is under target/ or the tests' TMPDIR is theirs: the gate's
# own ancestors excepted, given ten seconds to finish ending, any still there
# is named by pid and command line, killed, and fails the gate. Only the tests'
# TMPDIR is looked in, not all of .afkconvoy-tmp: a worker's own harness keeps
# its home there, beside the fast gate it runs.
left_behind() {
    ours=$$
    ancestors=" $$ "
    while [ "$ours" -gt 1 ] 2>/dev/null; do
        ours=$(ps -o ppid= -p "$ours" | tr -d ' ')
        [ -n "$ours" ] || break
        ancestors="$ancestors$ours "
    done
    waited=0
    while :; do
        if [ -d /proc/self ]; then
            pids=$(for p in /proc/[0-9]*; do
                for f in cwd exe; do
                    printf '%s %s\n' "${p#/proc/}" "$(readlink "$p/$f" 2>/dev/null)"
                done
            done)
        else
            pids=$(lsof -nPw -d cwd,txt -Fpn 2>/dev/null | awk '/^p/ { pid = substr($0, 2) } /^n/ { print pid, substr($0, 2) }')
        fi
        pids=$(printf '%s\n' "$pids" | awk -v target="$PWD/target/" -v tmp="$tests_tmpdir/" -v skip="$ancestors" '
            { path = substr($0, length($1) + 2) }
            (index(path, target) == 1 || index(path, tmp) == 1) && index(skip, " " $1 " ") == 0 { print $1 }' | sort -un)
        [ -n "$pids" ] || return 0
        [ "$waited" -lt 10 ] || break
        sleep 1
        waited=$((waited + 1))
    done
    echo "gate: the tests left these processes behind, in target/ or the tests' TMPDIR; each is killed, and the gate fails:" >&2
    # shellcheck disable=SC2086
    ps -o pid=,ppid=,stat=,etime=,command= -p "$(echo $pids | tr ' ' ,)" >&2 || true
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
    return 1
}
# Runs the tests nextest's filter `$2` picks under its profile `$1`, with
# the deep TMPDIR. Without nextest no test is told from another, and only the
# whole gate runs its tests, all of them, with cargo test.
run_tests() {
    mkdir -p "$tests_tmpdir"
    printf '*\n' >"$PWD/.afkconvoy-tmp/.gitignore"
    if [ -z "$nextest" ]; then
        TMPDIR="$tests_tmpdir" cargo test --workspace --quiet && tests_status=0 || tests_status=$?
        left_behind || [ "$tests_status" -ne 0 ] || tests_status=1
        [ "$tests_status" -eq 0 ] || exit "$tests_status"
        return
    fi
    test_log=$(mktemp -d "$PWD/.afkconvoy-tmp/nextest.XXXXXX")
    # Keep nextest's output live and retain its own exit status through tee.
    (if TMPDIR="$tests_tmpdir" cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level slow --show-progress none --no-tests=pass --profile "$1" -E "$2"; then
        echo 0 >"$test_log/status"
    else
        echo "$?" >"$test_log/status"
    fi) 2>&1 | tee "$test_log/output"
    tests_status=$(cat "$test_log/status")
    left_behind || [ "$tests_status" -ne 0 ] || tests_status=1
    if [ "$tests_status" -ne 0 ]; then
        if [ "$1" = fast ]; then
            check_fast_timeouts "$test_log/output"
        elif grep -q 'TIMEOUT' "$test_log/output"; then
            echo "gate: a test listed above as TIMEOUT ran too long: it was ended at the limit for one test, which .config/nextest.toml gives" >&2
        fi
        rm -rf "$test_log"
        exit "$tests_status"
    fi
    rm -rf "$test_log"
}

# A timeout under contention is not evidence that a test belongs in the slow
# gate. Check each one alone, only while the load average is below the host's
# logical CPU count. A failed check that is not a timeout is not a slow test.
host_load() {
    uptime | awk -F 'load averages?: ' 'NF > 1 { gsub(/,/, "", $2); split($2, a, " "); print a[1] }'
}
quiet_host() {
    awk -v load="$1" -v cpus="$2" 'BEGIN { exit !(load ~ /^[0-9]+([.][0-9]+)?$/ && cpus ~ /^[0-9]+$/ && cpus > 0 && load < cpus) }'
}
check_fast_timeouts() {
    timeouts=$(awk '/TIMEOUT[[:space:]]*\[/ { sub(/^.*TIMEOUT[[:space:]]*\[[^]]*\][[:space:]]*/, ""); if (NF >= 2) print $1, $2 }' "$1" | sort -u)
    [ -n "$timeouts" ] || return 0
    cpus=$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)
    load=$(host_load)
    if ! quiet_host "$load" "$cpus"; then
        echo "gate: host overloaded or load unavailable (load ${load:-unknown}, CPUs ${cpus:-unknown}); cannot classify TIMEOUT tests. Retry on a quiet host." >&2
        return 0
    fi
    printf '%s\n' "$timeouts" | while read -r binary name; do
        load=$(host_load)
        if ! quiet_host "$load" "$cpus"; then
            echo "gate: host overloaded (load ${load:-unknown}, CPUs $cpus); cannot classify $binary $name. Retry on a quiet host." >&2
            continue
        fi
        echo "gate: checking $binary $name alone against the 2s line (load $load, CPUs $cpus)" >&2
        check_log=$(mktemp "$PWD/.afkconvoy-tmp/check.XXXXXX")
        if TMPDIR="$tests_tmpdir" cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level fail --show-progress none --no-tests=fail --test-threads 1 --profile fast-check -E "binary_id(=$binary) & test(=$name)" >"$check_log" 2>&1; then
            after=$(host_load)
            if quiet_host "$after" "$cpus"; then
                echo "gate: $binary $name passed the 2s check; the earlier TIMEOUT was a load fault (load $load)." >&2
            else
                echo "gate: $binary $name passed the 2s check; host overloaded during it (load ${after:-unknown}, CPUs $cpus)." >&2
            fi
        elif grep -q 'TIMEOUT' "$check_log"; then
            after=$(host_load)
            if quiet_host "$after" "$cpus"; then
                echo "gate: $binary $name was still TIMEOUT at 2s on a quiet host. Name it in .config/slow-tests as its binary and name." >&2
            else
                echo "gate: host overloaded during the check (load ${after:-unknown}, CPUs $cpus); cannot classify $binary $name." >&2
            fi
        else
            echo "gate: checking $binary $name failed without a TIMEOUT:" >&2
            cat "$check_log" >&2
        fi
        rm -f "$check_log"
    done
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
# Every function of the core crate and of the harness, gate and landing
# crates it depends on (docs/adr/0058, docs/adr/0059, docs/adr/0060) is
# checked: none is taken on trust.
# Flux says so itself, in the summary that ends its run on a crate, so an
# exemption counts however it is written. Flux summarises every dependency:
# the core's summary is the one before cargo's last artifact message, the core
# being the crate checked and so the last one finished, and the harness
# crate's, the gate crate's and the landing crate's each the one before its
# own.
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
summaries=$(awk '
    /^\{/ { if (/"reason":"compiler-artifact"/) { if (/#afkconvoy-harness@/) harness = last; if (/#afkconvoy-gate@/) gate = last; if (/#afkconvoy-landing@/) landing = last; core = last; last = "" }; next }
    /Timing Report/ { timings = ""; in_timings = 1 }
    in_timings && !/^(───|Total running time:|Functions checked:|Min:|Max:|Mean:|Std\. Dev\.:|Top 5 Functions|• )/ { in_timings = 0 }
    in_timings { timings = timings $0 "\n"; next }
    { print > "/dev/stderr" }
    /^summary\./ { last = $0 }
    END {
        print harness
        print gate
        print landing
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
for crate in harness gate landing core; do
    case "$crate" in
    harness) summary=$(printf '%s\n' "$summaries" | sed -n 1p) ;;
    gate) summary=$(printf '%s\n' "$summaries" | sed -n 2p) ;;
    landing) summary=$(printf '%s\n' "$summaries" | sed -n 3p) ;;
    core) summary=$(printf '%s\n' "$summaries" | sed -n 4p) ;;
    esac
    case "$summary" in
    *"; 0 trusted; 0 ignored."*) ;;
    *)
        echo "gate: the $crate crate must not exempt anything from Flux, and Flux reports: ${summary:-no summary of the $crate crate}" >&2
        slowest
        exit 1
        ;;
    esac
done
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
