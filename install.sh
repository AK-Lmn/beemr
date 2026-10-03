#!/bin/sh
# Install beemr on macOS or Linux:
#   curl -fsSL https://raw.githubusercontent.com/osmanahmadxai/beemr/main/install.sh | sh
#
# Downloads the right binary for this machine from GitHub Releases, verifies
# its SHA-256 checksum, installs it to ~/.local/bin, then runs `beemr setup`
# to name this device.
set -eu

REPO="${BEEMR_REPO:-osmanahmadxai/beemr}"
INSTALL_DIR="${BEEMR_INSTALL_DIR:-$HOME/.local/bin}"

fail() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "this installer needs '$1'"; }
need curl
need uname

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) fail "unsupported OS $(uname -s). On Windows, use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) fail "unsupported CPU $(uname -m)" ;;
esac

echo "This installer will:"
echo "  - download beemr for $os ($arch) from GitHub and verify its checksum"
echo "  - install it to $INSTALL_DIR and add that folder to your PATH if needed"
echo "  - ask you to name this device (nothing runs in the background)"
echo "Uninstall any time: curl -fsSL https://raw.githubusercontent.com/$REPO/main/uninstall.sh | sh"
echo

asset="beemr-$os-$arch"
base="https://github.com/$REPO/releases/latest/download"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading beemr for $os ($arch)…"
curl -fsSL "$base/$asset" -o "$tmp/beemr" || fail "download failed"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || fail "checksum download failed"

expected="$(grep " $asset\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/beemr" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$tmp/beemr" | cut -d' ' -f1)"
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch; not installing"

mkdir -p "$INSTALL_DIR"
mv "$tmp/beemr" "$INSTALL_DIR/beemr"
chmod 755 "$INSTALL_DIR/beemr"
echo "Installed beemr to $INSTALL_DIR/beemr"

# Make sure future terminals can find it.
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    case "$(basename "${SHELL:-sh}")" in
      zsh) rc="$HOME/.zshrc" ;;
      bash) rc="$HOME/.bashrc" ;;
      *) rc="$HOME/.profile" ;;
    esac
    grep -qs "$INSTALL_DIR" "$rc" || printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$rc"
    echo "Added $INSTALL_DIR to your PATH (open a new terminal to use 'beemr' everywhere)."
    ;;
esac

echo
# Ask for the device name on the terminal even though this script is piped into sh.
if [ -r /dev/tty ] && [ -w /dev/tty ] && (: < /dev/tty) 2>/dev/null; then
  "$INSTALL_DIR/beemr" setup < /dev/tty
else
  "$INSTALL_DIR/beemr" setup
fi
