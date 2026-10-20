#!/bin/sh
# Sidefleet installer: puts a single `sidefleet` binary in ~/.local/bin (no sudo).
# Usage: curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh
set -e

REPO="${SIDEFLEET_REPO:-foodaka/side-fleet}"
BIN_DIR="${SIDEFLEET_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$BIN_DIR"

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$os-$arch" in
  darwin-arm64|darwin-aarch64) target=aarch64-apple-darwin ;;
  darwin-x86_64)               target=x86_64-apple-darwin ;;
  linux-x86_64)                target=x86_64-unknown-linux-gnu ;;
  linux-aarch64|linux-arm64)   target=aarch64-unknown-linux-gnu ;;
  *) echo "Unsupported platform: $os $arch"; exit 1 ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
url="https://github.com/$REPO/releases/latest/download/sidefleet-$target.tar.gz"

echo "Sidefleet installer"
if curl -fsSL "$url" -o "$tmp/sidefleet.tar.gz" 2>/dev/null; then
  tar -xzf "$tmp/sidefleet.tar.gz" -C "$tmp"
  install -m 755 "$tmp/sidefleet" "$BIN_DIR/sidefleet"
  echo "Installed prebuilt binary to $BIN_DIR/sidefleet"
else
  echo "No prebuilt binary for $target yet; building from source (a few minutes)."
  if ! command -v cargo >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
      PATH="$HOME/.cargo/bin:$PATH"
    else
      echo "Installing Rust (rustup)…"
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
      PATH="$HOME/.cargo/bin:$PATH"
    fi
  fi
  cargo install --git "https://github.com/$REPO" --locked --root "$tmp/root" sidefleet
  install -m 755 "$tmp/root/bin/sidefleet" "$BIN_DIR/sidefleet"
  echo "Built and installed to $BIN_DIR/sidefleet"
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo
    echo "Add $BIN_DIR to your PATH, e.g. for zsh:"
    echo "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.zshrc && export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

echo
echo "Next:"
echo "  on the machine that will host the page:   sidefleet serve"
echo "  on each machine that should run tasks:    sidefleet connect --control http://<host>:8080 --token <token>"
