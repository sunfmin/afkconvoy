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
: > "$tmp/repo/.config/slow-tests"
cat > "$tmp/repo/verify.sh" <<'MOCK'
#!/bin/sh
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact","package_id":"path+file:///repo/crates/harness#afkconvoy-harness@0.0.0"}'
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact","package_id":"path+file:///repo/crates/gate#afkconvoy-gate@0.0.0"}'
echo 'summary. 0 errors; 0 trusted; 0 ignored.'
echo '{"reason":"compiler-artifact"}'
MOCK
cat > "$tmp/home/.cargo/bin/cargo" <<'MOCK'
#!/bin/sh
case "$*" in
*'nextest --version'*|*'--no-run'*|fmt*|clippy*) exit 0 ;;
*'nextest run'*'--profile fast -E'*)
    if [ "$CASE" = failure ]; then echo 'FAIL [   0.100s] afkconvoy::cli tick::example' >&2; else echo 'TIMEOUT [   6.000s] afkconvoy::cli tick::example' >&2; fi
    exit 1 ;;
*'nextest run'*)
    if [ "$CASE" = pass ]; then exit 0; fi
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
