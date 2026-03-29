#!/bin/sh
set -e

REPO="thinkany-ai/codeany"
BINARY="codeany"
# Install dir: always user-owned, no sudo needed
INSTALL_DIR="${CODEANY_INSTALL_DIR:-$HOME/.local/bin}"

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
    OS="$(uname -s 2>/dev/null || echo unknown)"
    ARCH="$(uname -m 2>/dev/null || echo unknown)"

    case "$OS" in
        Linux)  OS="linux" ;;
        Darwin) OS="darwin" ;;
        MINGW*|MSYS*|CYGWIN*) OS="windows" ;;
        *) error "Unsupported OS: $OS" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)   ARCH="amd64" ;;
        arm64|aarch64)  ARCH="arm64" ;;
        armv7l|armv6l)  ARCH="arm" ;;
        *) error "Unsupported architecture: $ARCH" ;;
    esac

    PLATFORM="${OS}_${ARCH}"
}

# Get latest release version
get_latest_version() {
    if command -v curl >/dev/null 2>&1; then
        VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    elif command -v wget >/dev/null 2>&1; then
        VERSION=$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    else
        error "curl or wget is required"
    fi

    [ -n "$VERSION" ] || error "Could not get latest version from GitHub. Check: https://github.com/${REPO}/releases"
}

# Download binary, return path via stdout
download_binary() {
    EXT=""
    [ "$OS" = "windows" ] && EXT=".exe"
    FILENAME="${BINARY}_${PLATFORM}${EXT}"
    URL="https://github.com/${REPO}/releases/download/${VERSION}/${FILENAME}"

    TMP_DIR="$(mktemp -d)"
    TMP_FILE="${TMP_DIR}/${BINARY}${EXT}"

    info "Downloading ${BINARY} ${VERSION} for ${PLATFORM}..."

    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$URL" -o "$TMP_FILE" \
            || error "Download failed. Check releases: https://github.com/${REPO}/releases"
    else
        wget -qO "$TMP_FILE" "$URL" \
            || error "Download failed. Check releases: https://github.com/${REPO}/releases"
    fi

    chmod +x "$TMP_FILE"
    printf "%s" "$TMP_FILE"  # Only path goes to stdout
}

# Install binary to user dir (no sudo)
install_binary() {
    TMP_FILE="$1"
    mkdir -p "$INSTALL_DIR"
    mv "$TMP_FILE" "${INSTALL_DIR}/${BINARY}"
    success "Installed to ${INSTALL_DIR}/${BINARY}"
}

# Detect current shell config file
detect_shell_config() {
    SHELL_NAME="$(basename "${SHELL:-sh}")"
    case "$SHELL_NAME" in
        zsh)  echo "$HOME/.zshrc" ;;
        bash)
            # macOS bash uses ~/.bash_profile, Linux uses ~/.bashrc
            if [ "$(uname -s)" = "Darwin" ]; then
                echo "$HOME/.bash_profile"
            else
                echo "$HOME/.bashrc"
            fi
            ;;
        fish) echo "$HOME/.config/fish/config.fish" ;;
        *)    echo "$HOME/.profile" ;;
    esac
}

# Add INSTALL_DIR to PATH in shell config if not already there
add_to_path() {
    # Check if already in PATH
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) 
            return 0  # Already in PATH
            ;;
    esac

    SHELL_CONFIG="$(detect_shell_config)"
    PATH_SNIPPET="export PATH=\"${INSTALL_DIR}:\$PATH\""

    # fish uses different syntax
    if [ "$(basename "${SHELL:-sh}")" = "fish" ]; then
        PATH_SNIPPET="fish_add_path ${INSTALL_DIR}"
    fi

    # Check if already in config file
    if [ -f "$SHELL_CONFIG" ] && grep -qF "$INSTALL_DIR" "$SHELL_CONFIG" 2>/dev/null; then
        return 0
    fi

    # Append to config file
    {
        printf "\n# Added by CodeAny installer\n"
        printf "%s\n" "$PATH_SNIPPET"
    } >> "$SHELL_CONFIG"

    success "Added ${INSTALL_DIR} to PATH in ${SHELL_CONFIG}"
    warn "Run: source ${SHELL_CONFIG}   (or open a new terminal)"
    NEED_SOURCE="$SHELL_CONFIG"
}

# Verify installation
verify_install() {
    if PATH="${INSTALL_DIR}:${PATH}" command -v "$BINARY" >/dev/null 2>&1; then
        INSTALLED_VERSION=$(PATH="${INSTALL_DIR}:${PATH}" "$BINARY" --version 2>/dev/null || echo "installed")
        success "✓ ${BINARY} ${INSTALLED_VERSION} ready"
    fi
}

main() {
    printf "\n" >&2
    info "Installing CodeAny - AI Coding Agent"
    printf "\n" >&2

    detect_platform
    info "Platform: ${PLATFORM}"
    info "Install dir: ${INSTALL_DIR}"

    get_latest_version
    info "Latest version: ${VERSION}"

    TMP_FILE=$(download_binary)
    install_binary "$TMP_FILE"
    add_to_path
    verify_install

    printf "\n" >&2
    success "🎉 CodeAny ${VERSION} installed!"
    printf "\n" >&2

    if [ -n "$NEED_SOURCE" ]; then
        printf "  Activate PATH for this session:\n" >&2
        printf "    ${YELLOW}source ${NEED_SOURCE}${NC}\n" >&2
        printf "\n" >&2
    fi

    printf "  Set your API key and start:\n" >&2
    printf "    ${YELLOW}export ANTHROPIC_API_KEY=\"sk-ant-...\"${NC}\n" >&2
    printf "    ${GREEN}codeany${NC}\n" >&2
    printf "\n" >&2
    printf "  Self-update anytime:\n" >&2
    printf "    ${GREEN}codeany update${NC}\n" >&2
    printf "\n" >&2
}

main
