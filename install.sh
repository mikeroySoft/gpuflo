#!/bin/sh
# GPUFlo binary installer: downloads the latest GitHub release archive,
# verifies its SHA-256 checksum, and installs the single binary into a
# user-owned directory. No root, no groups, no udev rules, no services.
#
#   curl -fsSL https://raw.githubusercontent.com/mikeroySoft/gpuflo/main/install.sh | sh
#
# Override the destination with GPUFLO_INSTALL_DIR (default: ~/.local/bin).
set -eu

REPO="mikeroySoft/gpuflo"
TARGET="x86_64-unknown-linux-gnu"
DEST="${GPUFLO_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

[ "$(uname -s)" = "Linux" ] || fail "prebuilt archives are Linux-only; use: cargo install gpuflo --locked"
[ "$(uname -m)" = "x86_64" ] || fail "prebuilt archives are x86_64-only; use: cargo install gpuflo --locked"
command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" |
    sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
[ -n "${TAG}" ] || fail "could not determine the latest release tag"

NAME="gpuflo-${TAG}-${TARGET}"
BASE="https://github.com/${REPO}/releases/download/${TAG}"

TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT INT TERM

printf 'Downloading %s ...\n' "${NAME}.tar.gz"
curl -fsSL -o "${TMP}/${NAME}.tar.gz" "${BASE}/${NAME}.tar.gz"
curl -fsSL -o "${TMP}/SHA256SUMS" "${BASE}/SHA256SUMS"

(
    cd "${TMP}"
    grep -F " ${NAME}.tar.gz" SHA256SUMS >checksum || fail "archive missing from SHA256SUMS"
    sha256sum -c checksum >/dev/null || fail "checksum verification failed"
)

tar -xzf "${TMP}/${NAME}.tar.gz" -C "${TMP}" gpuflo
mkdir -p "${DEST}"
install -m 755 "${TMP}/gpuflo" "${DEST}/gpuflo"

printf 'Installed %s\n' "${DEST}/gpuflo"
"${DEST}/gpuflo" --version

case ":${PATH}:" in
*":${DEST}:"*) ;;
*) printf 'Note: %s is not in PATH\n' "${DEST}" ;;
esac
