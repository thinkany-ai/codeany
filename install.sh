#!/bin/sh
set -e

REPO="thinkany-ai/codeany"
BINARY="codeany"
INSTALL_DIR="/usr/local/bin"

# Colors (output to stderr so they don't pollute stdout captures)
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { printf "${BLUE}[codeany]${NC} %s\n" "$1" >&2; }
success() { printf "${GREEN}[codeany]${NC} %s\n" "$1" >&2; }
warn()    { printf "${YELLOW}[codeany]${NC} %s\n" "$1" >&2; }
error()   { printf "${RED}[codeany]${NC} ERROR: %s\n" "$1" >&2; exit 1; }

# Detect OS and arch
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)  OS="linux" ;;
        Darwin) OS="darwin" ;;
        MINGW*|MSYS*|CYGWIN*) OS="windows" ;;
        *) error "Unsupported OS: $OS" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)  ARCH="amd64" ;;
        arm64|aarch64) ARCH="arm64" ;;
        armv7l)        ARCH="arm" ;;
        *) error "Unsupported architecture: $ARCH" ;;
    esac

    PLATFORM="${OS}_${ARCH}"
}

# Get latest release version from GitHub
get_latest_version() {
    if command -v curl >/dev/null 2>&1; then
        VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    elif command -v wget >/dev/null 2>&1; then
        VERSION=$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    else
        error "curl or wget is required"
    fi

    if [ -z "$VERSION" ]; then
        error "Could not determine latest version. Check https://github.com/${REPO}/releases"
    fi
}

# Download binary to a temp file, print path to stdout
download_binary() {
    FILENAME="${BINARY}_${PLATFORM}"
    if [ "$OS" = "windows" ]; then
        FILENAME="${FILENAME}.exe"
    fi

    URL="https://github.com/${REPO}/releases/download/${VERSION}/${FILENAME}"
    TMP_DIR="$(mktemp -d)"
    TMP_FILE="${TMP_DIR}/${BINARY}"

    info "Downloading ${BINARY} ${VERSION} for ${PLATFORM}..."
    info "URL: ${URL}"

    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$URL" -o "$TMP_FILE" || error "Download failed. Is v${VERSION} released? Check: https://github.com/${REPO}/releases"
    else
        wget -qO "$TMP_FILE" "$URL" || error "Download failed. Check: https://github.com/${REPO}/releases"
    fi

    chmod +x "$TMP_FILE"
    # Only print the path to stdout — all other output above goes to stderr
    printf "%s" "$TMP_FILE"
}

# Install binary
install_binary() {
    TMP_FILE="$1"

    if [ -w "$INSTALL_DIR" ]; then
        mv "$TMP_FILE" "${INSTALL_DIR}/${BINARY}"
        success "Installed to ${INSTALL_DIR}/${BINARY}"
    elif command -v sudo >/dev/null 2>&1; then
        sudo mv "$TMP_FILE" "${INSTALL_DIR}/${BINARY}"
        success "Installed to ${INSTALL_DIR}/${BINARY} (with sudo)"
    else
        LOCAL_BIN="$HOME/.local/bin"
        mkdir -p "$LOCAL_BIN"
        mv "$TMP_FILE" "${LOCAL_BIN}/${BINARY}"
        success "Installed to ${LOCAL_BIN}/${BINARY}"
        warn "Add to PATH: export PATH=\"\$HOME/.local/bin:\$PATH\""
    fi
}

# Verify installation
verify_install() {
    if command -v "$BINARY" >/dev/null 2>&1; then
        INSTALLED_VERSION=$("$BINARY" --version 2>/dev/null || echo "unknown")
        success "✓ ${BINARY} installed! (${INSTALLED_VERSION})"
    else
        warn "${BINARY} installed but not in PATH. Restart your shell or run: hash -r"
    fi
}

main() {
    printf "\n" >&2
    info "Installing CodeAny - AI Coding Agent"
    printf "\n" >&2

    detect_platform
    info "Platform: ${PLATFORM}"

    get_latest_version
    info "Latest version: ${VERSION}"

    TMP_FILE=$(download_binary)
    install_binary "$TMP_FILE"
    verify_install

    printf "\n" >&2
    success "🎉 CodeAny is ready!"
    printf "\n" >&2
    printf "  Set your API key and start:\n" >&2
    printf "  ${YELLOW}export ANTHROPIC_API_KEY=\"sk-ant-...\"${NC}\n" >&2
    printf "  ${GREEN}codeany${NC}\n" >&2
    printf "\n" >&2
    printf "  Docs: https://github.com/${REPO}\n" >&2
    printf "\n" >&2
}

main
