#!/bin/sh
# Installs the traceqa binary from a GitHub release, on Linux and macOS.
#
#   curl -LsSf https://raw.githubusercontent.com/thearpankumar/TraceQA/master/scripts/install.sh | sh
#
# With options:
#
#   curl -LsSf https://raw.githubusercontent.com/thearpankumar/TraceQA/master/scripts/install.sh | sh -s -- --version v0.2.0
#
#   --version TAG       TRACEQA_VERSION          A release tag such as v0.2.0. Default: the newest release.
#   --install-dir DIR   TRACEQA_INSTALL_DIR      Where traceqa goes. Default: $XDG_BIN_HOME, else ~/.local/bin.
#   --no-modify-path    TRACEQA_NO_MODIFY_PATH=1 Leave your shell's profile alone; only say what to add to PATH.
#
# TRACEQA_DOWNLOAD_BASE and TRACEQA_GITHUB_API point it at a mirror (and at a local server in the tests).
#
# The archive is checked against the release's SHA256SUMS.txt before anything is installed. The script is
# all functions until its last line, so a download cut off halfway runs nothing. On Windows, use
# scripts/install.ps1.

set -eu

REPO="thearpankumar/TraceQA"
DOCS="https://github.com/$REPO#installation"

say() {
    printf 'traceqa-installer: %s\n' "$1"
}

err() {
    printf 'traceqa-installer: error: %s\n' "$1" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || err "this needs '$1', which is not installed"
}

usage() {
    sed -n '2,21p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
    printf 'Options: --version TAG, --install-dir DIR, --no-modify-path, --help\n'
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || err "--version needs a tag, such as v0.2.0"
                VERSION=$2
                shift 2
                ;;
            --version=*)
                VERSION=${1#*=}
                shift
                ;;
            --install-dir)
                [ $# -ge 2 ] || err "--install-dir needs a folder"
                INSTALL_DIR=$2
                shift 2
                ;;
            --install-dir=*)
                INSTALL_DIR=${1#*=}
                shift
                ;;
            --no-modify-path)
                NO_MODIFY_PATH=1
                shift
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                err "unknown option '$1' (see --help)"
                ;;
        esac
    done
}

# HTTPS only. Plain http is allowed for this machine alone, which is what the tests serve from.
protocols_for() {
    case "$1" in
        https://*) echo "=https" ;;
        http://127.0.0.1:* | http://127.0.0.1/* | http://localhost:* | http://localhost/*) echo "=http,https" ;;
        *) err "refusing to download from '$1': only https addresses are allowed" ;;
    esac
}

# fetch URL FILE: download URL into FILE, or to standard output when FILE is "-".
fetch() {
    _protocols=$(protocols_for "$1")
    if command -v curl >/dev/null 2>&1; then
        curl --proto "$_protocols" --tlsv1.2 --fail --silent --show-error --location --retry 3 \
            --header "Accept: application/vnd.github+json" --output "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        if [ "$_protocols" = "=https" ]; then
            wget --https-only --quiet --tries=3 --header="Accept: application/vnd.github+json" -O "$2" "$1"
        else
            wget --quiet --tries=3 --header="Accept: application/vnd.github+json" -O "$2" "$1"
        fi
    else
        err "this needs curl or wget, and neither is installed"
    fi
}

detect_platform() {
    _os=$(uname -s)
    _arch=$(uname -m)
    case "$_os" in
        Linux) _os=linux ;;
        Darwin)
            _os=macos
            # A shell running under Rosetta reports x86_64 on Apple silicon.
            if [ "$_arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
                _arch=arm64
            fi
            ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            err "on Windows, run this in PowerShell: irm https://raw.githubusercontent.com/$REPO/master/scripts/install.ps1 | iex"
            ;;
        *) err "there is no prebuilt traceqa for $_os; build it from source: $DOCS" ;;
    esac
    case "$_arch" in
        x86_64 | amd64) _arch=x86_64 ;;
        aarch64 | arm64) _arch=arm64 ;;
    esac
    PLATFORM="$_os-$_arch"
    case "$PLATFORM" in
        linux-x86_64)
            if ldd --version 2>&1 | grep -qi musl; then
                err "the Linux build needs glibc, and this system uses musl (Alpine?); build it from source: $DOCS"
            fi
            ;;
        macos-arm64) ;;
        *) err "there is no prebuilt traceqa for $PLATFORM yet (there are linux-x86_64, macos-arm64 and windows-x86_64); build it from source: $DOCS" ;;
    esac
}

# Until 1.0 every release is a pre-release, which GitHub's /releases/latest leaves out: ask for the
# newest release of any kind instead.
resolve_version() {
    if [ -z "$VERSION" ]; then
        _api="${TRACEQA_GITHUB_API:-https://api.github.com}/repos/$REPO/releases?per_page=1"
        _json=$(fetch "$_api" -) ||
            err "could not ask GitHub for the newest release (offline, or rate limited?); name one with --version v0.2.0"
        # One "key": value per line, whether GitHub sends the JSON indented or not. tag_name is never
        # the first key of a release, so it always follows a comma.
        VERSION=$(printf '%s\n' "$_json" | tr ',' '\n' | sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
        [ -n "$VERSION" ] || err "TraceQA has no release yet; build it from source: $DOCS"
    fi
    case "$VERSION" in
        v*) ;;
        *) VERSION="v$VERSION" ;;
    esac
    case "$VERSION" in
        *[!A-Za-z0-9._-]*) err "'$VERSION' is not a release tag" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1
    else
        err "this needs sha256sum, shasum or openssl to check the download, and none is installed"
    fi
}

download_and_verify() {
    ARCHIVE="traceqa-$VERSION-$PLATFORM.tar.gz"
    _base="${TRACEQA_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}/$VERSION"
    say "downloading $ARCHIVE"
    fetch "$_base/$ARCHIVE" "$TMP/$ARCHIVE" || err "could not download $_base/$ARCHIVE"
    fetch "$_base/SHA256SUMS.txt" "$TMP/SHA256SUMS.txt" || err "could not download $_base/SHA256SUMS.txt"
    _want=$(awk -v name="$ARCHIVE" '$2 == name || $2 == "*" name { print $1; exit }' "$TMP/SHA256SUMS.txt")
    [ -n "$_want" ] || err "SHA256SUMS.txt of $VERSION has no line for $ARCHIVE"
    _have=$(sha256_of "$TMP/$ARCHIVE")
    _want=$(printf '%s' "$_want" | tr 'A-F' 'a-f')
    [ "$_have" = "$_want" ] || err "checksum mismatch for $ARCHIVE (expected $_want, got $_have); nothing was installed"
    say "checksum verified"
}

install_binary() {
    mkdir -p "$TMP/unpacked"
    tar -xzf "$TMP/$ARCHIVE" -C "$TMP/unpacked"
    [ -f "$TMP/unpacked/traceqa" ] || err "$ARCHIVE holds no traceqa binary"
    mkdir -p "$INSTALL_DIR" || err "could not create $INSTALL_DIR"
    # Copy next to the target, then rename over it: a traceqa that is running keeps working.
    cp "$TMP/unpacked/traceqa" "$INSTALL_DIR/.traceqa.new" || err "could not write to $INSTALL_DIR"
    chmod 755 "$INSTALL_DIR/.traceqa.new"
    mv -f "$INSTALL_DIR/.traceqa.new" "$INSTALL_DIR/traceqa"
    if ! INSTALLED_VERSION=$("$INSTALL_DIR/traceqa" --version 2>&1); then
        rm -f "$INSTALL_DIR/traceqa"
        err "the downloaded traceqa does not run on this system: $INSTALLED_VERSION"
    fi
    say "installed $INSTALLED_VERSION to $INSTALL_DIR/traceqa"
}

# add_to_profile FILE LINE: append LINE to FILE between markers, once.
add_to_profile() {
    if [ -f "$1" ] && grep -F -x "$2" "$1" >/dev/null 2>&1; then
        return 0
    fi
    mkdir -p "$(dirname "$1")"
    printf '\n# >>> traceqa installer >>>\n%s\n# <<< traceqa installer <<<\n' "$2" >>"$1"
    say "added $INSTALL_DIR to PATH in $1"
}

modify_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) return 0 ;;
    esac
    _export="export PATH=\"$INSTALL_DIR:\$PATH\""
    if [ "$NO_MODIFY_PATH" = 1 ]; then
        say "$INSTALL_DIR is not on your PATH; add this line to your shell's profile: $_export"
        return 0
    fi
    case "$(basename "${SHELL:-sh}")" in
        zsh) add_to_profile "${ZDOTDIR:-$HOME}/.zshrc" "$_export" ;;
        bash)
            add_to_profile "$HOME/.bashrc" "$_export"
            # macOS terminals start login shells, which read .bash_profile and not .bashrc.
            if [ "$(uname -s)" = Darwin ]; then
                add_to_profile "$HOME/.bash_profile" "$_export"
            fi
            ;;
        fish) add_to_profile "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/traceqa.fish" "fish_add_path -g \"$INSTALL_DIR\"" ;;
        *) add_to_profile "$HOME/.profile" "$_export" ;;
    esac
    say "open a new terminal to use traceqa, or run now: $_export"
}

next_steps() {
    if command -v uv >/dev/null 2>&1; then
        say "next, install a browser: traceqa install chromium   (then check with: traceqa doctor)"
    else
        say "next, install uv (https://docs.astral.sh/uv/getting-started/installation/), then: traceqa install chromium"
    fi
}

main() {
    VERSION=${TRACEQA_VERSION:-}
    INSTALL_DIR=${TRACEQA_INSTALL_DIR:-${XDG_BIN_HOME:-${HOME:?HOME is not set}/.local/bin}}
    NO_MODIFY_PATH=${TRACEQA_NO_MODIFY_PATH:-0}
    parse_args "$@"
    for _tool in uname tar mkdir cp chmod mv rm awk sed tr cut; do
        need "$_tool"
    done
    detect_platform
    resolve_version
    TMP=$(mktemp -d 2>/dev/null || mktemp -d -t traceqa) || err "could not create a temporary folder"
    trap 'rm -rf "$TMP"' EXIT
    trap 'exit 130' HUP INT TERM
    download_and_verify
    install_binary
    modify_path
    next_steps
}

main "$@"
