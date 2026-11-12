#!/bin/sh
# Exercise `verify.sh install` and `--toolchain` with a stand-in brew: the
# check passes only on sunfmin/tap/flux-toolchain installed and the newest the
# tap has, install installs it or upgrades it and leaves the newest alone, and
# a run puts the tap's cargo-flux before any an older install left in
# ~/.local/bin or ~/.cargo/bin. Nothing is fetched, and nothing is pinned or
# built: there is no pin left to read.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
mkdir -p "$root/.afkconvoy-tmp"
tmp=$(mktemp -d "$root/.afkconvoy-tmp/verify-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/repo/crates/core" "$tmp/home/.cargo/bin" "$tmp/home/.local/bin" \
    "$tmp/bin" "$tmp/brew/bin" "$tmp/prefix/bin"
cp "$root/verify.sh" "$tmp/repo/verify.sh"
log="$tmp/log"
: >"$log"
fail() {
    echo "verify-install: $1" >&2
    echo "--- log" >&2
    cat "$log" >&2
    echo "--- stderr" >&2
    cat "$tmp/err" >&2
    exit 1
}
: >"$tmp/err"
for f in verify.sh crates/core/Cargo.toml; do
    grep -qE '^\[package\.metadata\.checker\]|^(z3|fixpoint|stack) = ' "$root/$f" &&
        fail "$f still pins z3, fixpoint or stack"
done
grep -qE 'curl|xtask|stack |git clone|unzip|tar ' "$root/verify.sh" &&
    fail "verify.sh still downloads or builds a tool itself"

# The stand-in brew keeps what is installed in $BREW/installed, and the
# newest the tap has, as last read, in $BREW/newest; `update` reads the tap's
# newest from $BREW/tap.
cat >"$tmp/brew/bin/brew" <<'MOCK'
#!/bin/sh
echo "brew $*" >>"$LOG"
f=sunfmin/tap/flux-toolchain
case "$*" in
"update --quiet") cp "$BREW/tap" "$BREW/newest" ;;
"list --versions $f") [ -f "$BREW/installed" ] && echo "flux-toolchain $(cat "$BREW/installed")" ;;
"outdated --quiet $f")
    [ -f "$BREW/installed" ] && [ "$(cat "$BREW/installed")" != "$(cat "$BREW/newest")" ] && echo "$f"
    exit 0
    ;;
"install --quiet $f" | "upgrade --quiet $f") cp "$BREW/newest" "$BREW/installed" ;;
"--prefix $f") echo "$PREFIX" ;;
*) exit 1 ;;
esac
MOCK
cat >"$tmp/bin/rustup" <<'MOCK'
#!/bin/sh
echo stand-in-toolchain
MOCK
# Cargo runs `cargo flux` as the first cargo-flux on the PATH, as rustup's does.
cat >"$tmp/home/.cargo/bin/cargo" <<'MOCK'
#!/bin/sh
[ "$1" = flux ] && exec cargo-flux "$@"
MOCK
for d in "$tmp/home/.local/bin" "$tmp/home/.cargo/bin"; do
    printf '#!/bin/sh\necho "the cargo-flux of an older install ran: $0" >&2; exit 1\n' >"$d/cargo-flux"
done
printf '#!/bin/sh\necho "tap cargo-flux $*" >>"$LOG"\n' >"$tmp/prefix/bin/cargo-flux"
chmod +x "$tmp/brew/bin/brew" "$tmp/bin/"* "$tmp/home/.cargo/bin/"* "$tmp/home/.local/bin/"* "$tmp/prefix/bin/"*
echo 2026.11.12 >"$tmp/brew/tap"
: >"$tmp/brew/newest"
verify() {
    HOME="$tmp/home" BREW="$tmp/brew" PREFIX="$tmp/prefix" LOG="$log" \
        PATH="$tmp/home/.local/bin:$tmp/home/.cargo/bin:$tmp/brew/bin:$tmp/bin:/usr/bin:/bin" \
        sh "$tmp/repo/verify.sh" "$@" 2>"$tmp/err"
}
asked() { grep -qxF -- "$1" "$log" || fail "never asked: $1"; }
never() { grep -qxF -- "$1" "$log" && fail "asked: $1" || true; }

verify --toolchain && fail "a host without the toolchain passed the check"
grep -q "sunfmin/tap/flux-toolchain is not installed" "$tmp/err" || fail "the check did not say the toolchain is missing"
verify && fail "a run without the toolchain checked something"

: >"$log"
verify install || fail "install on a host without the toolchain failed"
asked "brew update --quiet"
asked "brew install --quiet sunfmin/tap/flux-toolchain"
verify --toolchain || fail "the check failed after install"

: >"$log"
verify install || fail "a second install failed"
never "brew install --quiet sunfmin/tap/flux-toolchain"
never "brew upgrade --quiet sunfmin/tap/flux-toolchain"

# The tap moves on: the check reads it fresh and refuses the older one, and
# install upgrades it.
echo 2026.11.13 >"$tmp/brew/tap"
verify --toolchain && fail "the check passed an older toolchain than the tap's newest"
grep -q "flux-toolchain 2026.11.12 is not the newest the tap has" "$tmp/err" ||
    fail "the check did not say the toolchain is older than the tap's"
: >"$log"
verify install || fail "install of the tap's newer toolchain failed"
asked "brew upgrade --quiet sunfmin/tap/flux-toolchain"
never "brew install --quiet sunfmin/tap/flux-toolchain"
verify --toolchain || fail "the check failed after the upgrade"

# A run is the tap's cargo-flux, ahead of the older ones on the PATH.
: >"$log"
verify free_slots || fail "a run with the newest toolchain failed"
asked "tap cargo-flux flux check --only-check=def:free_slots"

# Without brew, the check says where it looked; a host with one in
# /opt/homebrew cannot be without it.
if [ ! -x /opt/homebrew/bin/brew ]; then
    mv "$tmp/brew/bin/brew" "$tmp/brew/brew"
    verify --toolchain && fail "the check passed with no brew"
    grep -q "no brew is on the PATH" "$tmp/err" || fail "the check did not say brew is missing"
fi
