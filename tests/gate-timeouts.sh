#!/bin/sh
# Exercise gate.sh with nextest and host readings controlled at its boundary.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
mkdir -p "$root/.afkconvoy-tmp"
tmp=$(mktemp -d "$root/.afkconvoy-tmp/gate-timeouts.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/home/.cargo/bin" "$tmp/repo/.config"
cp "$root/gate.sh" "$tmp/repo/gate.sh"
cp "$root/.config/nextest.toml" "$tmp/repo/.config/nextest.toml"
cp "$root/.config/process-bound" "$tmp/repo/.config/process-bound"
: > "$tmp/repo/.config/slow-tests"
# The mock repo is a repository of its own, or git would find this checkout's.
# With no base branch the change cannot be read, and every test is touched.
git init -q -b main "$tmp/repo"
cat > "$tmp/repo/verify.sh" <<'MOCK'
#!/bin/sh
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact","package_id":"path+file:///repo/crates/harness#afkconvoy-harness@0.0.0"}'
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact","package_id":"path+file:///repo/crates/gate#afkconvoy-gate@0.0.0"}'
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact","package_id":"path+file:///repo/crates/landing#afkconvoy-landing@0.0.0"}'
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact"}'
MOCK
cat > "$tmp/home/.cargo/bin/cargo" <<'MOCK'
#!/bin/sh
echo "$*" >> "$HOME/ran"
case "$*" in
metadata*) echo "{\"target_directory\":\"$PWD/target\",\"version\":1}" ;;
*'nextest --version'*|*'--no-run'*|fmt*|clippy*) exit 0 ;;
*'nextest run'*'--profile fast-loaded -E'*)
    if [ "$CASE" = loaded-recovers ]; then exit 0; fi
    echo 'TIMEOUT [ 240.000s] afkconvoy::cli tick::example' >&2; exit 1 ;;
*'nextest run'*'--profile fast -E'*)
    case "$CASE" in slow-passed | slow-fixed) echo '        SLOW [   2.500s] afkconvoy::cli tick::example' >&2; exit 0 ;; esac
    if [ "$CASE" = failure ]; then echo 'FAIL [   0.100s] afkconvoy::cli tick::example' >&2; else echo 'TIMEOUT [   6.000s] afkconvoy::cli tick::example' >&2; fi
    exit 1 ;;
*'nextest run'*)
    if [ "$CASE" = pass ] || [ "$CASE" = slow-fixed ]; then exit 0; fi
    echo 'TIMEOUT [   2.000s] afkconvoy::cli tick::example' >&2; exit 1 ;;
esac
exit 0
MOCK
cat > "$tmp/home/.cargo/bin/uptime" <<'MOCK'
#!/bin/sh
printf 'load averages: %s 1.00 1.00\n' "$FAKE_LOAD"
MOCK
cat > "$tmp/home/.cargo/bin/getconf" <<'MOCK'
#!/bin/sh
echo 4
MOCK
chmod +x "$tmp/repo/verify.sh" "$tmp/home/.cargo/bin/"*
absent() {
    if grep -q "$1" "$tmp/out"; then
        cat "$tmp/out" >&2
        exit 1
    fi
}
for scenario in loaded slow pass failure; do
    case "$scenario" in loaded) load=40.00 ;; *) load=1.00 ;; esac
    HOME="$tmp/home" FAKE_LOAD="$load" CASE="$scenario" sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 && exit 1
    if [ "$scenario" = loaded ]; then
        absent 'Name it in .config/slow-tests'
        grep -qi 'overload' "$tmp/out"
        grep -q '40.00' "$tmp/out"
    elif [ "$scenario" = slow ]; then
        grep -q 'Name it in .config/slow-tests' "$tmp/out"
    elif [ "$scenario" = pass ]; then
        absent 'Name it in .config/slow-tests'
        grep -q 'load fault' "$tmp/out"
    else
        absent 'Name it in .config/slow-tests'
        absent 'load fault'
    fi
done
# A fast-gate test that passed past the 2s line is checked alone: still past
# it on a quiet host, it fails the gate; under the line alone, it does not.
HOME="$tmp/home" FAKE_LOAD=1.00 CASE=slow-passed sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 && exit 1
grep -q 'Name it in .config/slow-tests' "$tmp/out"
HOME="$tmp/home" FAKE_LOAD=1.00 CASE=slow-fixed sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 || {
    cat "$tmp/out" >&2
    exit 1
}
grep -q 'load fault' "$tmp/out"
# On a loaded host it is flagged, and the gate passes.
HOME="$tmp/home" FAKE_LOAD=40.00 CASE=slow-passed sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 || {
    cat "$tmp/out" >&2
    exit 1
}
grep -q 'flagged: afkconvoy::cli tick::example' "$tmp/out"
# A fast gate red only on TIMEOUTs on a loaded host runs its tests again under
# the limit for one test: green there, the gate is green and each TIMEOUT is
# flagged; red there, the gate is red (sunfmin/control#1183).
HOME="$tmp/home" FAKE_LOAD=40.00 CASE=loaded-recovers sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 || {
    cat "$tmp/out" >&2
    exit 1
}
grep -q 'they run again under the limit for one test' "$tmp/out"
grep -q 'flagged: afkconvoy::cli tick::example timed out in the fast gate' "$tmp/out"
grep -q -- '--profile fast-loaded' "$tmp/home/ran"
HOME="$tmp/home" FAKE_LOAD=40.00 CASE=loaded sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 && exit 1
grep -q 'they run again under the limit for one test' "$tmp/out"
# A FAIL is no load fault: nothing runs again, loaded or not; nor does a
# TIMEOUT on a quiet host.
for load in 40.00 1.00; do
    rm -f "$tmp/home/ran"
    HOME="$tmp/home" FAKE_LOAD=$load CASE=failure sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 && exit 1
    absent 'run again under the limit'
    if grep -q -- '--profile fast-loaded' "$tmp/home/ran"; then exit 1; fi
done
rm -f "$tmp/home/ran"
HOME="$tmp/home" FAKE_LOAD=1.00 CASE=slow sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 && exit 1
absent 'run again under the limit'
# A test past the 2s line that the change under test did not touch warns and
# is named in one open issue; a touched one fails the gate (sunfmin/control#1307).
cat > "$tmp/home/.cargo/bin/afkconvoy" <<'MOCK'
#!/bin/sh
echo "$*" >> "$HOME/afkconvoy-ran"
case "$*" in
'issue list'*) if [ -f "$HOME/open-issue" ]; then echo '{"issues":[{"number":7}]}'; else echo '{"issues":[]}'; fi ;;
'issue create'*) : > "$HOME/open-issue"; echo '{"action":"created","number":7}' ;;
'issue view'*) printf '{"body":"%s","comments":[]}\n' "$(cat "$HOME/open-issue")" ;;
'issue comment'*) echo '{"action":"commented","number":7}' ;;
esac
exit 0
MOCK
chmod +x "$tmp/home/.cargo/bin/afkconvoy"
cd "$tmp/repo"
git config user.email gate@example.invalid
git config user.name gate
mkdir -p src
printf 'fn example() {}\nfn other() {\n    0;\n}\nfn third() {}\n' > src/lib.rs
git add -A src && git commit -q -m base
git checkout -q -b change
printf 'fn example() {}\nfn other() {\n    0;\n}\nfn third() { 1; }\n' > src/lib.rs
git commit -q -am 'touch another test'
cd "$root"
HOME="$tmp/home" FAKE_LOAD=1.00 CASE=slow sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 || {
    cat "$tmp/out" >&2
    exit 1
}
grep -q 'afkconvoy::cli tick::example is past the 2s line and was not touched by this change' "$tmp/out"
absent 'Name it in .config/slow-tests'
grep -q '^issue create' "$tmp/home/afkconvoy-ran"
grep -q -- '--label ready-for-agent' "$tmp/home/afkconvoy-ran"
# A second sighting adds to the open issue, never a second one.
rm -f "$tmp/home/afkconvoy-ran"
HOME="$tmp/home" FAKE_LOAD=1.00 CASE=slow sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 || exit 1
if grep -q '^issue create' "$tmp/home/afkconvoy-ran"; then exit 1; fi
rm -f "$tmp/home/afkconvoy-ran"
# A test the change modifies, by its body, still fails the gate.
cd "$tmp/repo"
printf 'fn example() {\n    2;\n}\nfn other() {\n    0;\n}\nfn third() { 1; }\n' > src/lib.rs
git commit -q -am 'touch the slow test'
cd "$root"
HOME="$tmp/home" FAKE_LOAD=1.00 CASE=slow sh "$tmp/repo/gate.sh" fast >"$tmp/out" 2>&1 && exit 1
grep -q 'Name it in .config/slow-tests' "$tmp/out"
absent 'was not touched by this change'
