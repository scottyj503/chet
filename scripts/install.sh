#!/bin/sh
# Chet installer — downloads the latest release binary for your platform.
#
# Usage:
#   curl -sSf https://raw.githubusercontent.com/scottyj503/chet/main/scripts/install.sh | sh
#
# Environment variables:
#   CHET_INSTALL_DIR  — override install directory (default: ~/.chet/bin)
#   CHET_VERSION      — install a specific version (default: latest)

set -eu

REPO="scottyj503/chet"
INSTALL_DIR="${CHET_INSTALL_DIR:-$HOME/.chet/bin}"

main() {
    detect_platform
    get_version
    download_and_install
    post_install
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)  OS_TARGET="unknown-linux-musl" ;;
        Darwin) OS_TARGET="apple-darwin" ;;
        *)
            echo "Error: unsupported OS: $OS" >&2
            exit 1
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)   ARCH_TARGET="x86_64" ;;
        aarch64|arm64)   ARCH_TARGET="aarch64" ;;
        *)
            echo "Error: unsupported architecture: $ARCH" >&2
            exit 1
            ;;
    esac

    TARGET="${ARCH_TARGET}-${OS_TARGET}"
    echo "Detected platform: $TARGET"
}

get_version() {
    if [ -n "${CHET_VERSION:-}" ]; then
        VERSION="$CHET_VERSION"
    else
        VERSION="$(curl -sSf "https://api.github.com/repos/$REPO/releases/latest" \
            | grep '"tag_name"' \
            | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    fi

    if [ -z "$VERSION" ]; then
        echo "Error: could not determine latest version" >&2
        exit 1
    fi

    echo "Installing chet $VERSION"
}

download_and_install() {
    URL="https://github.com/$REPO/releases/download/$VERSION/chet-$TARGET.tar.gz"
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    echo "Downloading $URL"
    curl -sSfL "$URL" -o "$TMPDIR/chet.tar.gz"

    tar xzf "$TMPDIR/chet.tar.gz" -C "$TMPDIR"

    mkdir -p "$INSTALL_DIR"
    mv "$TMPDIR/chet" "$INSTALL_DIR/chet"
    chmod +x "$INSTALL_DIR/chet"

    # Remove macOS quarantine attribute if present
    if [ "$OS" = "Darwin" ]; then
        xattr -d com.apple.quarantine "$INSTALL_DIR/chet" 2>/dev/null || true
    fi

    echo "Installed chet to $INSTALL_DIR/chet"
}

post_install() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            echo "chet is ready! Run: chet --version"
            ;;
        *)
            echo ""
            echo "Add chet to your PATH:"
            echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
            echo ""
            echo "Add the line above to your shell profile (~/.bashrc, ~/.zshrc, etc.)"
            ;;
    esac
}

main
