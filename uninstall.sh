#!/bin/sh
# Uninstall beemr installed with install.sh (macOS and Linux):
#   curl -fsSL https://raw.githubusercontent.com/osmanahmadxai/beemr/main/uninstall.sh | sh
#
# Stops and removes the background service and the beemr binary. Your
# identity, contacts and inbox are kept unless BEEMR_PURGE=1 is set.
set -u
INSTALL_DIR="${BEEMR_INSTALL_DIR:-$HOME/.local/bin}"
BIN="$INSTALL_DIR/beemr"

if [ -x "$BIN" ]; then
  "$BIN" daemon uninstall 2>/dev/null && echo "Stopped the background service and removed it from startup."
  rm -f "$BIN" && echo "Removed $BIN"
else
  echo "beemr is not installed in $INSTALL_DIR (packages: use apt/dnf/snap/brew remove)."
fi

if [ "${BEEMR_PURGE:-}" = "1" ]; then
  case "$(uname -s)" in
    Darwin) DATA="$HOME/Library/Application Support/beemr" ;;
    *) DATA="${XDG_CONFIG_HOME:-$HOME/.config}/beemr" ;;
  esac
  rm -rf "$DATA" && echo "Deleted your beemr identity, contacts and inbox ($DATA)."
else
  echo "Your identity, contacts and inbox were kept. To delete them too, run with BEEMR_PURGE=1."
fi
echo "The PATH line added to your shell profile is harmless; remove it by hand if you like."
