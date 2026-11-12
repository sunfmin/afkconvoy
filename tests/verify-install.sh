#!/bin/sh
# Exercise `verify.sh install` with stand-ins for every download and build:
# it installs what the pins in Cargo.toml and crates/core/Cargo.toml name,
# and fixpoint from liquid-fixpoint's nightly release, asks for a pin's new
# version once it changes, leaves alone a tool already the one needed, and
# refuses a platform the nightly has no fixpoint for. Nothing is fetched.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
mkdir -p "$root/.afkconvoy-tmp"
tmp=$(mktemp -d "$root/.afkconvoy-tmp/verify-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/repo/crates/core" "$tmp/home/.cargo/bin" "$tmp/bin"
cp "$root/verify.sh" "$tmp/repo/verify.sh"
cp "$root/Cargo.toml" "$tmp/repo/Cargo.toml"
cp "$root/crates/core/Cargo.toml" "$tmp/repo/crates/core/Cargo.toml"
log="$tmp/log"
: >"$log"
# Each stand-in logs how it was asked, then does just enough of the real
# tool's work for the next step and for the check to see the result.
cat >"$tmp/bin/rustup" <<'MOCK'
#!/bin/sh
echo stand-in-toolchain
MOCK
cat >"$tmp/bin/curl" <<'MOCK'
#!/bin/sh
echo "curl $*" >>"$LOG"
out= url=
while [ $# -gt 0 ]; do
    case "$1" in -o) out=$2; shift ;; https://*) url=$1 ;; esac
    shift
done
case "$url" in
https://api.github.com/repos/Z3Prover/z3/releases/tags/z3-*)
    v=${url##*/z3-}
    for p in arm64-osx-13.3 x64-osx-13.3 arm64-glibc-2.38 x64-glibc-2.39; do
        printf '      "browser_download_url": "https://github.com/Z3Prover/z3/releases/download/z3-%s/z3-%s-%s.zip"\n' "$v" "$v" "$p"
    done
    ;;
*) echo "$url" >"$out" ;;
esac
MOCK
cat >"$tmp/bin/unzip" <<'MOCK'
#!/bin/sh
# unzip -q <zip> -d <dir>: the release holds z3-<version>-<platform>/bin/z3.
name=$(basename "$(cat "$2")" .zip)
v=${name#z3-}
v=${v%%-*}
mkdir -p "$4/$name/bin"
printf '#!/bin/sh\necho "Z3 version %s - 64 bit"\n' "$v" >"$4/$name/bin/z3"
chmod +x "$4/$name/bin/z3"
MOCK
cat >"$tmp/bin/tar" <<'MOCK'
#!/bin/sh
# tar -xzf <tarball> -C <dir>: liquid-fixpoint's nightly holds fixpoint alone.
echo "tar $*" >>"$LOG"
printf '#!/bin/sh\necho "fixpoint 0.9.6.3.8 (69e585b4c0ffee)"\n' >"$4/fixpoint"
MOCK
cat >"$tmp/bin/uname" <<'MOCK'
#!/bin/sh
case "$1" in -s) echo "${UNAME_S:-Darwin}" ;; -m) echo "${UNAME_M:-arm64}" ;; esac
MOCK
cat >"$tmp/bin/git" <<'MOCK'
#!/bin/sh
echo "git $*" >>"$LOG"
case "$1" in
clone) mkdir -p "$4/.git" ;;
-C) case "$3" in checkout) echo "$6" >"$2/.git/checked-out" ;; esac ;;
esac
MOCK
cat >"$tmp/home/.cargo/bin/cargo" <<'MOCK'
#!/bin/sh
case "$*" in
'flux --version')
    [ -f "$HOME/.flux/rev" ] && echo "cargo-flux $(cut -c1-10 "$HOME/.flux/rev") (2026-10-09)"
    ;;
'xtask install')
    echo "cargo $* RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-}" >>"$LOG"
    mkdir -p "$HOME/.flux"
    cp .git/checked-out "$HOME/.flux/rev"
    ;;
esac
MOCK
chmod +x "$tmp/bin/"* "$tmp/home/.cargo/bin/cargo"
verify() {
    HOME="$tmp/home" XDG_CACHE_HOME= TMPDIR="$tmp" PATH="$tmp/bin:/usr/bin:/bin" LOG="$log" \
        sh "$tmp/repo/verify.sh" "$@" 2>"$tmp/err"
}
fail() {
    echo "verify-install: $1" >&2
    echo "--- log" >&2
    cat "$log" >&2
    echo "--- stderr" >&2
    cat "$tmp/err" >&2
    exit 1
}
asked() { grep -qF -- "$1" "$log" || fail "never asked: $1"; }
# Sets a pin in the copy's Cargo.toml: `key = "value"` on its line.
pin() { sed -i.bak "s|^$1 = .*|$1 = \"$2\"|" "$tmp/repo/crates/core/Cargo.toml"; }
pin z3 7.7.7
sed -i.bak 's|^\(flux-rs = .*rev = "\)[0-9a-f]*"|\1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"|' "$tmp/repo/Cargo.toml"
grep -qE '^(fixpoint|stack) = ' "$tmp/repo/crates/core/Cargo.toml" &&
    fail "crates/core/Cargo.toml still pins fixpoint or stack"

verify --toolchain && fail "a host with none of the three passed the check"
grep -q "fixpoint reports no version" "$tmp/err" || fail "the check did not say fixpoint is missing"

# A platform the nightly builds no fixpoint for is refused, before anything
# is installed, and nothing is built from source.
for platform in Darwin-x86_64 Linux-aarch64; do
    : >"$log"
    (UNAME_S=${platform%-*} UNAME_M=${platform#*-} && export UNAME_S UNAME_M && verify install) &&
        fail "install on $platform, which the nightly has no fixpoint for, passed"
    grep -q "nightly release has no fixpoint for ${platform%-*} ${platform#*-}" "$tmp/err" ||
        fail "install on $platform did not say the nightly has no fixpoint for it"
    [ ! -s "$log" ] || fail "install on $platform fetched or built something"
done

verify install || fail "install on a host with none of the three failed"
asked "curl -fsSL https://api.github.com/repos/Z3Prover/z3/releases/tags/z3-7.7.7"
asked "/z3-7.7.7/z3-7.7.7-"
asked "git clone --quiet https://github.com/flux-rs/flux $tmp/home/.cache/src/flux"
asked "git -C $tmp/home/.cache/src/flux checkout --quiet --force aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
grep -qx "cargo xtask install RUSTUP_TOOLCHAIN=" "$log" ||
    fail "Flux was not built under the toolchain its own source names"
asked "https://github.com/ucsd-progsys/liquid-fixpoint/releases/download/nightly/fixpoint-aarch64-apple-darwin.tar.gz"
grep -qE '^stack |/src/liquid-fixpoint|clone .*ucsd-progsys/liquid-fixpoint' "$log" &&
    fail "install built fixpoint from source"
[ -x "$tmp/home/.local/bin/fixpoint" ] || fail "install put no fixpoint in ~/.local/bin"
grep -q "verify: installed fixpoint 0.9.6.3.8 (69e585b4c0ffee)" "$tmp/err" ||
    fail "install did not say which fixpoint the nightly gave"
for tool in z3 cargo-flux fixpoint; do
    grep -q "verify: installed $tool " "$tmp/err" || fail "install did not say how long $tool took"
done
verify --toolchain || fail "the check failed after install"

: >"$log"
began=$(date +%s)
verify install || fail "a second install failed"
[ ! -s "$log" ] || fail "a second install with all three pinned fetched or built something"
[ $(($(date +%s) - began)) -lt 10 ] || fail "a second install took 10s or more"

# One pin changed: install asks for its new version, and for nothing else.
pin z3 8.8.8
verify --toolchain && fail "the check passed with z3 at its old pin"
verify install || fail "install of a changed z3 pin failed"
asked "z3-8.8.8"
grep -qv 'z3' "$log" && fail "install of a changed z3 pin did more than fetch z3"
sed -i.bak 's|^\(flux-rs = .*rev = "\)[0-9a-f]*"|\1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"|' "$tmp/repo/Cargo.toml"
: >"$log"
verify install || fail "install of a changed Flux revision failed"
asked "checkout --quiet --force bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
grep -q 'clone' "$log" && fail "install cloned Flux again instead of reusing its clone"
grep -qE '^(curl|tar) ' "$log" && fail "install of a changed Flux revision fetched z3 or fixpoint"
verify --toolchain || fail "the check failed after installing the changed pins"

# Any fixpoint that reports a version passes the check: it has no pin.
printf '#!/bin/sh\necho "fixpoint 9.9.9 (0000000)"\n' >"$tmp/home/.local/bin/fixpoint"
verify --toolchain || fail "the check refused a fixpoint of another version"
