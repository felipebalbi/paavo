#!/usr/bin/env bash
#
# setup-paavo.sh
#
# Provisioning script for a terminal-only Debian Trixie (13) host that will run
# paavo. Idempotent: safe to re-run.
#
# What this does:
#   - Installs base packages: build deps, hardware/embedded tooling, monitoring,
#     git/curl/etc.
#   - Creates a system user `paavo` (no login shell prompt, but with bash).
#   - Installs rustup + stable toolchain as the `paavo` user.
#   - Switches networking from ifupdown to systemd-networkd + systemd-resolved,
#     dropping in a simple DHCP profile for any wired interface.
#   - Hardens openssh-server (key-only, no root, no password auth).
#   - Configures nftables to allow SSH (22), paavod's HTTP API (8080), and
#     paavo-web's UI (8081); drops everything else inbound.
#   - Enables unattended security upgrades.
#
# Usage:
#   sudo ./setup-paavo.sh
#
# Re-run after editing constants below to update config in place.

set -euo pipefail

# ---------- Configuration ----------------------------------------------------

PAAVO_USER="paavo"
PAAVO_HOME="/home/${PAAVO_USER}"
PAAVO_HTTP_PORT="8080"     # paavod HTTP API (paavo-cli talks to this)
PAAVO_WEB_PORT="8081"      # paavo-web read-only UI
SSH_PORT="22"
RUST_TOOLCHAIN="stable"

# Cross-compilation targets to install for the paavo user.
# Add more here (one per line) as paavo grows to support new boards.
#   thumbv8m.main-none-eabihf  -> Cortex-M33/M35P with FPU (RP2350, nRF54L, etc.)
RUST_TARGETS=(
    "thumbv8m.main-none-eabihf"
)

# Optional: drop an authorized_keys file here before running and the script
# will install it for the paavo user. Leave empty to skip.
PAAVO_AUTHORIZED_KEYS_SRC=""

# ---------- Helpers ----------------------------------------------------------

log()  { printf '\033[1;34m[paavo-setup]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[paavo-setup]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[paavo-setup]\033[0m %s\n' "$*" >&2; exit 1; }

require_root() {
    if [[ "$(id -u)" -ne 0 ]]; then
        die "Must be run as root (try: sudo $0)"
    fi
}

require_debian_trixie() {
    if [[ ! -r /etc/os-release ]]; then
        die "Cannot detect OS (no /etc/os-release)"
    fi
    # shellcheck disable=SC1091
    . /etc/os-release
    if [[ "${ID:-}" != "debian" ]]; then
        warn "Detected ID=${ID:-unknown}; this script targets Debian. Continuing anyway."
    fi
    if [[ "${VERSION_CODENAME:-}" != "trixie" ]]; then
        warn "Detected codename '${VERSION_CODENAME:-unknown}'; this script targets Debian 13 (trixie). Continuing anyway."
    fi
}

# ---------- Steps ------------------------------------------------------------

install_packages() {
    log "Updating apt and installing base packages"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -y
    apt-get upgrade -y

    apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg jq vim less \
        git \
        build-essential pkg-config cmake clang \
        libssl-dev libudev-dev libusb-1.0-0-dev \
        openssh-server \
        nftables \
        systemd-resolved \
        unattended-upgrades apt-listchanges \
        htop iotop lsof tcpdump iproute2 \
        sudo
}

create_paavo_user() {
    if id -u "${PAAVO_USER}" >/dev/null 2>&1; then
        log "User ${PAAVO_USER} already exists"
    else
        log "Creating system user ${PAAVO_USER}"
        useradd --system --create-home --shell /bin/bash "${PAAVO_USER}"
    fi

    # paavo can talk to USB/serial devices without root (useful for embedded work)
    for grp in dialout plugdev; do
        if getent group "${grp}" >/dev/null 2>&1; then
            usermod -aG "${grp}" "${PAAVO_USER}"
        fi
    done

    # Install authorized_keys if provided
    if [[ -n "${PAAVO_AUTHORIZED_KEYS_SRC}" && -f "${PAAVO_AUTHORIZED_KEYS_SRC}" ]]; then
        log "Installing authorized_keys for ${PAAVO_USER}"
        install -d -m 700 -o "${PAAVO_USER}" -g "${PAAVO_USER}" "${PAAVO_HOME}/.ssh"
        install -m 600 -o "${PAAVO_USER}" -g "${PAAVO_USER}" \
            "${PAAVO_AUTHORIZED_KEYS_SRC}" "${PAAVO_HOME}/.ssh/authorized_keys"
    fi
}

install_rustup_for_paavo() {
    if sudo -u "${PAAVO_USER}" test -x "${PAAVO_HOME}/.cargo/bin/rustup"; then
        log "rustup already installed for ${PAAVO_USER}; updating toolchain"
        sudo -u "${PAAVO_USER}" -H bash -lc "${PAAVO_HOME}/.cargo/bin/rustup self update"
        sudo -u "${PAAVO_USER}" -H bash -lc "${PAAVO_HOME}/.cargo/bin/rustup update ${RUST_TOOLCHAIN}"
    else
        log "Installing rustup for ${PAAVO_USER}"
        sudo -u "${PAAVO_USER}" -H bash -lc "
            set -euo pipefail
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
                | sh -s -- -y --default-toolchain ${RUST_TOOLCHAIN} --profile minimal
        "
    fi

    # Install cross-compilation targets
    for target in "${RUST_TARGETS[@]}"; do
        log "Ensuring Rust target ${target} is installed"
        sudo -u "${PAAVO_USER}" -H bash -lc \
            "${PAAVO_HOME}/.cargo/bin/rustup target add --toolchain ${RUST_TOOLCHAIN} ${target}"
    done

    # Make sure cargo is on PATH for interactive shells
    local profile="${PAAVO_HOME}/.profile"
    if ! grep -q 'cargo/env' "${profile}" 2>/dev/null; then
        log "Adding cargo env to ${profile}"
        echo '. "$HOME/.cargo/env"' >> "${profile}"
        chown "${PAAVO_USER}:${PAAVO_USER}" "${profile}"
    fi
}

configure_networkd() {
    log "Switching to systemd-networkd + systemd-resolved"

    systemctl enable --now systemd-networkd.service systemd-resolved.service

    # Make resolv.conf point at resolved's stub
    if [[ ! -L /etc/resolv.conf ]] || \
       [[ "$(readlink -f /etc/resolv.conf)" != "/run/systemd/resolve/stub-resolv.conf" ]]; then
        ln -sf /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf
    fi

    # Drop in a simple DHCP profile for any wired interface
    install -d -m 755 /etc/systemd/network
    cat > /etc/systemd/network/10-wired-dhcp.network <<'EOF'
[Match]
Name=en*

[Network]
DHCP=yes
IPv6AcceptRA=yes

[DHCPv4]
UseDNS=yes
UseDomains=yes
EOF

    # Reload networkd config (don't bounce existing connection)
    networkctl reload || true

    # Remove ifupdown so it doesn't fight networkd. Only do this after networkd
    # is actually up and a wired interface is online, to avoid lockout.
    if dpkg -l ifupdown >/dev/null 2>&1; then
        if networkctl status --no-pager | grep -q 'State: routable'; then
            log "Removing ifupdown (networkd is routable)"
            apt-get purge -y ifupdown
        else
            warn "ifupdown still installed: networkd is not routable yet, skipping purge to avoid lockout"
        fi
    fi
}

harden_ssh() {
    log "Hardening sshd configuration"
    install -d -m 755 /etc/ssh/sshd_config.d
    cat > /etc/ssh/sshd_config.d/10-paavo.conf <<EOF
# Managed by setup-paavo.sh
Port ${SSH_PORT}
PermitRootLogin no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
PermitEmptyPasswords no
X11Forwarding no
ClientAliveInterval 300
ClientAliveCountMax 2
EOF

    if ! sshd -t; then
        die "sshd config test failed; refusing to reload"
    fi

    systemctl enable ssh
    systemctl reload ssh || systemctl restart ssh
}

configure_nftables() {
    log "Writing nftables ruleset (allow SSH ${SSH_PORT}, paavod ${PAAVO_HTTP_PORT}, paavo-web ${PAAVO_WEB_PORT})"

    cat > /etc/nftables.conf <<EOF
#!/usr/sbin/nft -f
# Managed by setup-paavo.sh

flush ruleset

table inet filter {
    chain input {
        type filter hook input priority filter; policy drop;

        # Always allow loopback
        iif "lo" accept

        # Allow established / related
        ct state established,related accept
        ct state invalid drop

        # ICMP / ICMPv6 (ping, PMTUD, neighbour discovery)
        ip protocol icmp accept
        ip6 nexthdr icmpv6 accept

        # SSH
        tcp dport ${SSH_PORT} ct state new accept

        # paavod HTTP API (paavo-cli)
        tcp dport ${PAAVO_HTTP_PORT} ct state new accept

        # paavo-web read-only UI
        tcp dport ${PAAVO_WEB_PORT} ct state new accept

        # Log + drop everything else (rate-limited)
        limit rate 5/second log prefix "nft-drop-in: " level info
    }

    chain forward {
        type filter hook forward priority filter; policy drop;
    }

    chain output {
        type filter hook output priority filter; policy accept;
    }
}
EOF

    # Validate before enabling
    if ! nft -c -f /etc/nftables.conf; then
        die "nftables ruleset failed validation"
    fi

    systemctl enable --now nftables.service
    systemctl reload nftables.service || nft -f /etc/nftables.conf
}

enable_unattended_upgrades() {
    log "Enabling unattended security upgrades"

    cat > /etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
EOF

    cat > /etc/apt/apt.conf.d/51paavo-unattended <<'EOF'
// Managed by setup-paavo.sh
Unattended-Upgrade::Origins-Pattern {
    "origin=Debian,codename=${distro_codename},label=Debian-Security";
};
Unattended-Upgrade::Automatic-Reboot "false";
Unattended-Upgrade::Remove-Unused-Dependencies "true";
EOF

    systemctl enable --now unattended-upgrades.service
}

print_summary() {
    cat <<EOF

==============================================================================
  paavo provisioning complete

  User:          ${PAAVO_USER}  (home: ${PAAVO_HOME})
  Rust:          $(sudo -u "${PAAVO_USER}" -H bash -lc 'rustc --version' 2>/dev/null || echo 'not yet on PATH; new shell needed')
  Rust targets:  ${RUST_TARGETS[*]}
  SSH:           port ${SSH_PORT}, key-only, no root
  Firewall:      nftables, inbound TCP ${SSH_PORT} + ${PAAVO_HTTP_PORT} (paavod) + ${PAAVO_WEB_PORT} (paavo-web) allowed
  Network:       systemd-networkd (DHCP on en*)
  Updates:       unattended security upgrades enabled

  Next steps:
    - Drop an SSH key into ${PAAVO_HOME}/.ssh/authorized_keys
      (or set PAAVO_AUTHORIZED_KEYS_SRC at the top of this script and re-run).
    - Log in as ${PAAVO_USER} and verify: rustup show, cargo --version.
    - Deploy paavo binaries; in paavo.toml set:
        [server] bind = "0.0.0.0:${PAAVO_HTTP_PORT}"   # paavod
        [web]    bind = "0.0.0.0:${PAAVO_WEB_PORT}"   # paavo-web
==============================================================================
EOF
}

# ---------- Main -------------------------------------------------------------

main() {
    require_root
    require_debian_trixie

    install_packages
    create_paavo_user
    install_rustup_for_paavo
    configure_networkd
    harden_ssh
    configure_nftables
    enable_unattended_upgrades

    print_summary
}

main "$@"
