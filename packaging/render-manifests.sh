#!/usr/bin/env bash
# Render the Homebrew formula, Scoop manifest and AUR PKGBUILD for a release:
#
#   packaging/render-manifests.sh 0.2.0 out/
#
# Checksums come from the release's SHA256SUMS on GitHub.
set -euo pipefail
VERSION="${1:?version, e.g. 0.2.0}"
OUT="${2:?output directory}"
REPO="osmanahmadxai/beemr"
BASE="https://github.com/$REPO/releases/download/v$VERSION"
SUMS="$(curl -fsSL "$BASE/SHA256SUMS")"
sum() { awk -v f="$1" '$2 == f {print $1}' <<<"$SUMS"; }
mkdir -p "$OUT/homebrew/Formula" "$OUT/scoop/bucket" "$OUT/aur/beemr-bin"

cat > "$OUT/homebrew/Formula/beemr.rb" <<RUBY
class Beemr < Formula
  desc "Peer-to-peer file sharing. No servers, no accounts, no setup"
  homepage "https://github.com/$REPO"
  version "$VERSION"
  license "MIT"

  on_macos do
    on_arm do
      url "$BASE/beemr-macos-aarch64"
      sha256 "$(sum beemr-macos-aarch64)"
    end
    on_intel do
      url "$BASE/beemr-macos-x86_64"
      sha256 "$(sum beemr-macos-x86_64)"
    end
  end

  on_linux do
    on_arm do
      url "$BASE/beemr-linux-aarch64"
      sha256 "$(sum beemr-linux-aarch64)"
    end
    on_intel do
      url "$BASE/beemr-linux-x86_64"
      sha256 "$(sum beemr-linux-x86_64)"
    end
  end

  def install
    bin.install Dir["beemr-*"].first => "beemr"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/beemr --version")
  end
end
RUBY

cat > "$OUT/scoop/bucket/beemr.json" <<JSON
{
    "version": "$VERSION",
    "description": "Peer-to-peer file sharing. No servers, no accounts, no setup.",
    "homepage": "https://github.com/$REPO",
    "license": "MIT",
    "architecture": {
        "64bit": {
            "url": "$BASE/beemr-windows-x86_64.exe#/beemr.exe",
            "hash": "$(sum beemr-windows-x86_64.exe)"
        }
    },
    "bin": "beemr.exe",
    "checkver": {
        "github": "https://github.com/$REPO"
    },
    "autoupdate": {
        "architecture": {
            "64bit": {
                "url": "https://github.com/$REPO/releases/download/v\$version/beemr-windows-x86_64.exe#/beemr.exe"
            }
        }
    }
}
JSON

cat > "$OUT/aur/beemr-bin/PKGBUILD" <<PKG
# Maintainer: Osman Ahmadzai <osmanahmadxai@gmail.com>
pkgname=beemr-bin
pkgver=$VERSION
pkgrel=1
pkgdesc="Peer-to-peer file sharing. No servers, no accounts, no setup."
arch=('x86_64' 'aarch64')
url="https://github.com/$REPO"
license=('MIT')
provides=('beemr')
conflicts=('beemr')
source_x86_64=("beemr-\$pkgver-x86_64::$BASE/beemr-linux-x86_64")
source_aarch64=("beemr-\$pkgver-aarch64::$BASE/beemr-linux-aarch64")
sha256sums_x86_64=('$(sum beemr-linux-x86_64)')
sha256sums_aarch64=('$(sum beemr-linux-aarch64)')

package() {
  install -Dm755 "beemr-\$pkgver-\$CARCH" "\$pkgdir/usr/bin/beemr"
}
PKG

cat > "$OUT/aur/beemr-bin/.SRCINFO" <<SRC
pkgbase = beemr-bin
	pkgdesc = Peer-to-peer file sharing. No servers, no accounts, no setup.
	pkgver = $VERSION
	pkgrel = 1
	url = https://github.com/$REPO
	arch = x86_64
	arch = aarch64
	license = MIT
	provides = beemr
	conflicts = beemr
	source_x86_64 = beemr-$VERSION-x86_64::$BASE/beemr-linux-x86_64
	sha256sums_x86_64 = $(sum beemr-linux-x86_64)
	source_aarch64 = beemr-$VERSION-aarch64::$BASE/beemr-linux-aarch64
	sha256sums_aarch64 = $(sum beemr-linux-aarch64)

pkgname = beemr-bin
SRC
echo "Rendered manifests for $VERSION into $OUT"
