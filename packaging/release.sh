#!/bin/sh
# What a release is made of, one step per subcommand, so the workflow
# (.github/workflows/release.yml) and a human at a terminal run the same thing.
#
#   release.sh version                   the version Cargo builds
#   release.sh check <tag>               fail unless <tag> is v<version>
#   release.sh package <target> <arch>   dist/afkconvoy-<version>-macos-<arch>.tar.gz
#                                        from target/<target>/release/afkconvoy
#   release.sh formula                   the Homebrew formula for what is in dist/
#
# DIST names another directory than dist/.
set -eu
cd "$(dirname "$0")/.."

REPO=https://github.com/sunfmin/afkconvoy
ARCHES="arm64 x86_64"
DIST=${DIST:-dist}

version() {
    cargo pkgid -p afkconvoy | sed 's/.*[#@]//'
}

archive() {
    echo "afkconvoy-$(version)-macos-$1.tar.gz"
}

sha() {
    shasum -a 256 "$1" | cut -d' ' -f1
}

case "${1:-}" in
version)
    version
    ;;
check)
    tag=${2:?usage: release.sh check <tag>}
    if [ "$tag" != "v$(version)" ]; then
        echo "release: the tag $tag is not v$(version), the version in Cargo.toml" >&2
        exit 1
    fi
    ;;
package)
    target=${2:?usage: release.sh package <target> <arch>}
    arch=${3:?usage: release.sh package <target> <arch>}
    built=target/$target/release/afkconvoy
    # `afkconvoy --version` must print the released version. A binary this
    # machine cannot run (Intel, with no Rosetta) is only checked for its arch.
    lipo -archs "$built" | grep -qx "$arch" || {
        echo "release: $built is not an $arch binary" >&2
        exit 1
    }
    if arch "-$arch" /usr/bin/true 2>/dev/null; then
        said=$(arch "-$arch" "$built" --version)
        if [ "$said" != "afkconvoy $(version)" ]; then
            echo "release: $built --version says '$said', not 'afkconvoy $(version)'" >&2
            exit 1
        fi
    else
        echo "release: this machine cannot run $arch: --version was not checked" >&2
    fi
    mkdir -p "$DIST"
    tar -czf "$DIST/$(archive "$arch")" -C "target/$target/release" afkconvoy
    echo "$DIST/$(archive "$arch")"
    ;;
formula)
    for arch in $ARCHES; do
        [ -f "$DIST/$(archive "$arch")" ] || {
            echo "release: $DIST/$(archive "$arch") is missing: package it first" >&2
            exit 1
        }
    done
    base=$REPO/releases/download/v$(version)
    cat <<FORMULA
class Afkconvoy < Formula
  desc "Fleet of AFK coding agents that works a GitHub backlog"
  homepage "$REPO"
  version "$(version)"

  depends_on "gh"
  depends_on "git"
  depends_on :macos

  on_arm do
    url "$base/$(archive arm64)"
    sha256 "$(sha "$DIST/$(archive arm64)")"
  end
  on_intel do
    url "$base/$(archive x86_64)"
    sha256 "$(sha "$DIST/$(archive x86_64)")"
  end

  def install
    bin.install "afkconvoy"
  end

  def caveats
    <<~EOS
      afkconvoy starts its workers through orca. Install it by its tap's
      name (the cask Homebrew itself calls orca is another program):
        brew install --cask stablyai/orca/orca
      \`gh\` must be logged in (\`gh auth login\`).
    EOS
  end

  test do
    assert_equal "afkconvoy #{version}", shell_output("#{bin}/afkconvoy --version").strip
  end
end
FORMULA
    ;;
*)
    sed -n '2,11p' "$0" >&2
    exit 2
    ;;
esac
