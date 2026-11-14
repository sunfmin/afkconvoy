#!/bin/sh
# Exercise how many tests gate.sh runs at a time, with nextest stood in for:
# two to each core `afkconvoy exclusive` gives it as NEXTEST_TEST_THREADS,
# and each profile's own count where it is given none.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
mkdir -p "$root/.afkconvoy-tmp"
tmp=$(mktemp -d "$root/.afkconvoy-tmp/gate-threads.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/home/.cargo/bin" "$tmp/repo/.config"
cp "$root/gate.sh" "$tmp/repo/gate.sh"
cp "$root/.config/nextest.toml" "$tmp/repo/.config/nextest.toml"
: > "$tmp/repo/.config/slow-tests"
cat > "$tmp/home/.cargo/bin/cargo" <<'MOCK'
#!/bin/sh
case "$*" in
*'nextest run'*) echo "$*" >"$NEXTEST_ARGS" ;;
esac
exit 0
MOCK
chmod +x "$tmp/home/.cargo/bin/cargo"
# Runs the slow gate given `$1` as NEXTEST_TEST_THREADS, none where it is
# empty, and says what nextest was asked to run its tests with.
threads() {
    rm -f "$tmp/args"
    if [ -n "$1" ]; then
        HOME="$tmp/home" NEXTEST_ARGS="$tmp/args" NEXTEST_TEST_THREADS="$1" sh "$tmp/repo/gate.sh" slow >"$tmp/out" 2>&1
    else
        (unset NEXTEST_TEST_THREADS; HOME="$tmp/home" NEXTEST_ARGS="$tmp/args" sh "$tmp/repo/gate.sh" slow >"$tmp/out" 2>&1)
    fi || {
        cat "$tmp/out" >&2
        exit 1
    }
    grep -o -- '--test-threads [0-9]*' "$tmp/args" || echo none
}
expect() {
    got=$(threads "$1")
    [ "$got" = "$2" ] || {
        echo "given NEXTEST_TEST_THREADS='$1', nextest ran with '$got', not '$2'" >&2
        exit 1
    }
}
expect 4 '--test-threads 8'
expect 8 '--test-threads 16'
expect 1 '--test-threads 2'
expect '' none
expect 0 none
expect many none
