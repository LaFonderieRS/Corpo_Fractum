#!/usr/bin/env bash
# =============================================================================
# RustDec — installation des dépendances système
# Distributions supportées : Debian/Ubuntu, Fedora/RHEL, Arch Linux
# Usage : bash install_deps.sh
# =============================================================================

set -euo pipefail

# ── Couleurs ──────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
RESET='\033[0m'

info()    { echo -e "${BLUE}[INFO]${RESET}  $*"; }
success() { echo -e "${GREEN}[OK]${RESET}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${RESET}  $*"; }
error()   { echo -e "${RED}[ERROR]${RESET} $*" >&2; }
die()     { error "$*"; exit 1; }

# ── Bannière ──────────────────────────────────────────────────────────────────

echo -e "${BOLD}"
echo "  ██████╗ ██╗   ██╗███████╗████████╗██████╗ ███████╗ ██████╗"
echo "  ██╔══██╗██║   ██║██╔════╝╚══██╔══╝██╔══██╗██╔════╝██╔════╝"
echo "  ██████╔╝██║   ██║███████╗   ██║   ██║  ██║█████╗  ██║"
echo "  ██╔══██╗██║   ██║╚════██║   ██║   ██║  ██║██╔══╝  ██║"
echo "  ██║  ██║╚██████╔╝███████║   ██║   ██████╔╝███████╗╚██████╗"
echo "  ╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝   ╚═════╝ ╚══════╝ ╚═════╝"
echo -e "${RESET}"
echo -e "  ${BLUE}Binary decompiler — dependency installer${RESET}"
echo "  ─────────────────────────────────────────────────────"
echo ""

# ── Détection de la distribution ─────────────────────────────────────────────

detect_distro() {
    if [ -f /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        echo "${ID}"
    elif command -v lsb_release &>/dev/null; then
        lsb_release -si | tr '[:upper:]' '[:lower:]'
    else
        echo "unknown"
    fi
}

DISTRO=$(detect_distro)
info "Distribution détectée : ${BOLD}${DISTRO}${RESET}"
echo ""

# ── Vérification des droits sudo ──────────────────────────────────────────────

need_sudo() {
    if [ "$(id -u)" -eq 0 ]; then
        # Déjà root — pas besoin de sudo
        SUDO=""
    elif command -v sudo &>/dev/null; then
        SUDO="sudo"
        info "sudo requis pour l'installation des paquets système."
    else
        die "sudo introuvable et script non exécuté en root. Installez sudo ou relancez en root."
    fi
}

need_sudo

# ── Paquets par distribution ──────────────────────────────────────────────────

install_debian_ubuntu() {
    info "Mise à jour de l'index APT…"
    $SUDO apt-get update -qq

    info "Installation des dépendances GTK4 / Cairo / build tools…"
    $SUDO apt-get install -y \
        build-essential \
        pkg-config \
        curl \
        git \
        libgtk-4-dev \
        libcairo2-dev \
        libglib2.0-dev \
        libpango1.0-dev \
        libgdk-pixbuf-2.0-dev \
        libgraphene-1.0-dev \
        libadwaita-1-dev \
        libcapstone-dev \
        clang \
        llvm \
        cmake

    success "Paquets système installés."
}

install_fedora() {
    info "Installation des dépendances GTK4 / Cairo / build tools…"
    $SUDO dnf install -y \
        gcc \
        gcc-c++ \
        make \
        pkgconf-pkg-config \
        curl \
        git \
        gtk4-devel \
        cairo-devel \
        glib2-devel \
        pango-devel \
        gdk-pixbuf2-devel \
        graphene-devel \
        libadwaita-devel \
        capstone-devel \
        clang \
        llvm \
        cmake

    success "Paquets système installés."
}

install_arch() {
    info "Synchronisation des dépôts pacman…"
    $SUDO pacman -Sy --noconfirm

    info "Installation des dépendances GTK4 / Cairo / build tools…"
    $SUDO pacman -S --noconfirm --needed \
        base-devel \
        pkgconf \
        curl \
        git \
        gtk4 \
        cairo \
        glib2 \
        pango \
        gdk-pixbuf2 \
        graphene \
        libadwaita \
        capstone \
        clang \
        llvm \
        cmake

    success "Paquets système installés."
}

# ── Dispatch selon la distro ──────────────────────────────────────────────────

case "${DISTRO}" in
    debian|ubuntu|linuxmint|pop|elementary|zorin|kali|raspbian)
        install_debian_ubuntu
        ;;
    fedora|rhel|centos|almalinux|rocky)
        install_fedora
        ;;
    arch|manjaro|endeavouros|garuda)
        install_arch
        ;;
    *)
        warn "Distribution '${DISTRO}' non reconnue automatiquement."
        warn "Installez manuellement : gtk4-devel cairo-devel capstone-devel pkg-config curl git"
        warn "Puis relancez ce script avec : DISTRO=debian bash install_deps.sh"
        die "Distribution non supportée."
        ;;
esac

echo ""

# ── Installation / vérification de Rust (rustup) ─────────────────────────────

install_rust() {
    if command -v rustup &>/dev/null; then
        info "rustup déjà installé. Mise à jour…"
        rustup update stable
    elif command -v cargo &>/dev/null; then
        info "cargo trouvé (Rust installé sans rustup)."
        RUST_VERSION=$(rustc --version | awk '{print $2}')
        info "Version actuelle : ${RUST_VERSION}"
    else
        info "Installation de Rust via rustup…"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain stable --profile default
        # Sourcer l'environnement rustup pour la suite du script
        # shellcheck disable=SC1091
        source "${HOME}/.cargo/env"
        success "Rust installé."
    fi

    # Vérification de la version minimale (1.78)
    RUST_VERSION=$(rustc --version 2>/dev/null | awk '{print $2}' || echo "0.0.0")
    REQUIRED="1.78.0"
    if printf '%s\n%s\n' "${REQUIRED}" "${RUST_VERSION}" | sort -V | head -1 | grep -qF "${REQUIRED}"; then
        success "Rust ${RUST_VERSION} ✓ (>= ${REQUIRED} requis)"
    else
        warn "Rust ${RUST_VERSION} détecté, ${REQUIRED} requis minimum."
        info "Mise à jour vers stable…"
        rustup update stable
    fi
}

install_rust
echo ""

# ── Vérification des outils de build ─────────────────────────────────────────

check_tool() {
    local name="$1"
    local cmd="${2:-$1}"
    if command -v "${cmd}" &>/dev/null; then
        success "${name} trouvé : $(command -v "${cmd}")"
    else
        warn "${name} introuvable — certaines fonctionnalités peuvent manquer."
    fi
}

info "Vérification des outils…"
check_tool "cargo"
check_tool "rustc"
check_tool "pkg-config"
check_tool "git"
check_tool "clang"
check_tool "cmake"
echo ""

# ── Vérification des bibliothèques GTK4 via pkg-config ───────────────────────

check_pkg() {
    local pkg="$1"
    local min_version="${2:-}"
    if pkg-config --exists "${pkg}" 2>/dev/null; then
        local version
        version=$(pkg-config --modversion "${pkg}" 2>/dev/null || echo "?")
        if [ -n "${min_version}" ]; then
            if pkg-config --atleast-version="${min_version}" "${pkg}" 2>/dev/null; then
                success "${pkg} ${version} ✓ (>= ${min_version})"
            else
                warn "${pkg} ${version} trouvé mais >= ${min_version} recommandé"
            fi
        else
            success "${pkg} ${version} ✓"
        fi
    else
        error "${pkg} introuvable via pkg-config — la compilation échouera."
        MISSING_PKGS="${MISSING_PKGS} ${pkg}"
    fi
}

MISSING_PKGS=""

info "Vérification des bibliothèques système…"
check_pkg "gtk4"          "4.12"
check_pkg "cairo"         "1.16"
check_pkg "glib-2.0"      "2.70"
check_pkg "pango"         "1.48"
check_pkg "gdk-pixbuf-2.0"
check_pkg "graphene-1.0"
echo ""

if [ -n "${MISSING_PKGS}" ]; then
    die "Bibliothèques manquantes :${MISSING_PKGS}. Réinstallez les paquets -dev correspondants."
fi

# ── Résumé final ──────────────────────────────────────────────────────────────

echo -e "${BOLD}  ✅  Toutes les dépendances sont prêtes !${RESET}"
echo ""
echo "  Pour compiler RustDec :"
echo -e "    ${BLUE}cd rustdec_mvp${RESET}"
echo -e "    ${BLUE}cargo build --release${RESET}"
echo ""
echo "  Pour lancer les tests :"
echo -e "    ${BLUE}cargo test${RESET}"
echo ""
echo "  Pour exécuter :"
echo -e "    ${BLUE}./target/release/rustdec${RESET}"
echo ""
