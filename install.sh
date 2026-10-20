#!/bin/sh
# Sidefleet installer: puts a single `sidefleet` binary in ~/.local/bin (no sudo).
# Public repo:   curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh
# From a clone:  git clone git@github.com:foodaka/side-fleet.git && sh side-fleet/install.sh
# Private repo:  gh api repos/foodaka/side-fleet/contents/install.sh -H "Accept: application/vnd.github.raw" | sh
#                (needs the GitHub CLI signed in: `gh auth login`)
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

# Running from inside a checkout? Then build from it and never touch GitHub.
checkout=""
for d in "$(cd "$(dirname "$0")" 2>/dev/null && pwd)" "$PWD"; do
  if [ -f "$d/crates/cli/Cargo.toml" ] && [ -f "$d/Cargo.lock" ]; then checkout="$d"; break; fi
done

ensure_rust() {
  if command -v cargo >/dev/null 2>&1; then return; fi
  if [ -x "$HOME/.cargo/bin/cargo" ]; then PATH="$HOME/.cargo/bin:$PATH"; return; fi
  echo "Installing Rust (rustup)…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
  PATH="$HOME/.cargo/bin:$PATH"
}

ensure_cc() {
  if command -v cc >/dev/null 2>&1; then return; fi
  if command -v apt-get >/dev/null 2>&1; then
    echo "Installing a C toolchain (build-essential, pkg-config)…"
    if [ "$(id -u)" = "0" ]; then apt-get update -qq && apt-get install -y -qq build-essential pkg-config >/dev/null
    else sudo apt-get update -qq && sudo apt-get install -y -qq build-essential pkg-config >/dev/null; fi
  elif command -v xcode-select >/dev/null 2>&1; then
    echo "Installing the Xcode command line tools…"; xcode-select --install || true
    echo "Re-run this script once that finishes."; exit 1
  else
    echo "A C compiler (cc) is needed to build. Install your platform's build tools and re-run."; exit 1
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
url="https://github.com/$REPO/releases/latest/download/sidefleet-$target.tar.gz"

echo "Sidefleet installer"
got=""
if [ -n "$checkout" ]; then
  echo "Building from the checkout at $checkout (a few minutes the first time)."
  ensure_cc; ensure_rust
  cargo install --path "$checkout/crates/cli" --locked --root "$tmp/root" --quiet
  install -m 755 "$tmp/root/bin/sidefleet" "$BIN_DIR/sidefleet"
  echo "Built and installed to $BIN_DIR/sidefleet"
  got=skip
elif curl -fsSL "$url" -o "$tmp/sidefleet.tar.gz" 2>/dev/null; then
  got=1
elif command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  # Private repo: the GitHub CLI can fetch release assets with your login.
  if gh release download --repo "$REPO" --pattern "sidefleet-$target.tar.gz" --dir "$tmp" >/dev/null 2>&1; then
    got=1
  fi
fi
if [ "$got" = "skip" ]; then
  :
elif [ -n "$got" ]; then
  tar -xzf "$tmp/sidefleet.tar.gz" -C "$tmp"
  install -m 755 "$tmp/sidefleet" "$BIN_DIR/sidefleet"
  echo "Installed prebuilt binary to $BIN_DIR/sidefleet"
else
  echo "No prebuilt binary for $target yet; building from source (a few minutes)."
  ensure_cc; ensure_rust
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    gh auth setup-git >/dev/null 2>&1 || true   # lets git clone a private repo with your gh login
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
echo "  on each machine that should run tasks:    sidefleet connect --control http://<host>:7373 --token <token>"
