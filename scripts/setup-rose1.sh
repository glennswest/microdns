#!/usr/bin/env bash
set -euo pipefail

# Setup MicroDNS containers on rose1.gw.lo (RouterOS)
# Based on netman deploy.sh patterns (slash-path RouterOS CLI)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ROSE_HOST="admin@rose1.gw.lo"
STORAGE="raid1"
TARBALL_DIR="${STORAGE}/tarballs"
IMAGE_DIR="${STORAGE}/images"
VOLUME_DIR="${STORAGE}/volumes"
TARBALL="microdns-arm64.tar"
DNS_SERVER="8.8.8.8"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

# Instance definitions: name ip/cidr gateway bridge mountprefix
# mountprefix maps to existing RouterOS mount list names (mdns.{domain})
INSTANCES=(
    "microdns-main 192.168.1.199/24 192.168.1.88 bridge-lan mdns.gw.lo"
    "microdns-g10 192.168.10.199/24 192.168.10.1 bridge mdns.g10.lo"
    "microdns-g11 192.168.11.199/24 192.168.11.1 bridge-boot mdns.g11.lo"
    "microdns-gt 192.168.200.199/24 192.168.200.1 bridge-gt mdns.gt.lo"
)

ros() {
    ssh $SSH_OPTS "$ROSE_HOST" "$1"
}

wait_state() {
    local name="$1" target="$2" max=30 i=0
    printf "  Waiting for %s -> %s " "$name" "$target"
    while [ $i -lt $max ]; do
        local output
        output=$(ros "/container/print" 2>/dev/null || true)
        if [ "$target" = "missing" ]; then
            if ! echo "$output" | grep -q "$name"; then
                printf "done\n"; return 0
            fi
        elif echo "$output" | grep "$name" | grep -qE "^\s*[0-9]+\s+${target}\s"; then
            printf "done\n"; return 0
        fi
        printf "."; i=$((i + 1)); sleep 2
    done
    printf " timeout!\n"; return 1
}

cmd_setup_bridge_gt() {
    echo "==> Creating bridge-gt (192.168.200.x)..."
    ros "/interface/bridge/add name=bridge-gt comment=container-mgmt-200x" 2>/dev/null || echo "  (already exists)"
    ros "/ip/address/add address=192.168.200.1/24 interface=bridge-gt" 2>/dev/null || echo "  (address already set)"
}

cmd_build_upload() {
    echo "==> Building tarball from docker image..."

    local src_tarball="${PROJECT_DIR}/microdns-arm64.tar.gz"
    if [ ! -f "$src_tarball" ]; then
        echo "ERROR: Run scripts/build-container.sh first (need ${src_tarball})"
        exit 1
    fi

    # RouterOS needs uncompressed tar from podman/docker save format
    # But we built a rootfs tarball — convert to docker save format
    echo "  Saving docker image to tar..."
    docker save microdns:arm64 -o "${PROJECT_DIR}/${TARBALL}"

    echo "  Uploading tarball to rose..."
    scp $SSH_OPTS "${PROJECT_DIR}/${TARBALL}" "${ROSE_HOST}:${TARBALL_DIR}/"

    rm -f "${PROJECT_DIR}/${TARBALL}"
    echo "==> Upload complete"
}

cmd_upload_configs() {
    echo "==> Uploading config files..."
    for inst_line in "${INSTANCES[@]}"; do
        read -r name _ _ _ _ <<< "$inst_line"
        local config_file="${PROJECT_DIR}/config/deploy/${name}.toml"

        if [ ! -f "$config_file" ]; then
            echo "  WARN: ${config_file} not found, skipping"
            continue
        fi

        echo "  Creating volume dirs for ${name}..."
        ros "/file/make-directory numbers=/${VOLUME_DIR}/${name}/config" 2>/dev/null || true
        ros "/file/make-directory numbers=/${VOLUME_DIR}/${name}/data" 2>/dev/null || true

        echo "  Uploading config for ${name}..."
        scp $SSH_OPTS "$config_file" "${ROSE_HOST}:${VOLUME_DIR}/${name}/config/microdns.toml"
    done
}

cmd_create_mounts() {
    echo "==> Creating container mounts..."
    for inst_line in "${INSTANCES[@]}"; do
        read -r name _ _ _ mprefix <<< "$inst_line"
        echo "  Mounts for ${name} (${mprefix})..."
        ros "/container/mounts/add list=${mprefix}.config src=/${VOLUME_DIR}/${name}/config dst=/etc/microdns" 2>/dev/null || true
        ros "/container/mounts/add list=${mprefix}.data src=/${VOLUME_DIR}/${name}/data dst=/data" 2>/dev/null || true
    done
}

cmd_create_containers() {
    echo "==> Creating containers..."
    for inst_line in "${INSTANCES[@]}"; do
        read -r name ip_cidr gateway bridge mprefix <<< "$inst_line"

        echo "  Creating veth for ${name} (${ip_cidr})..."
        ros "/interface/veth/add name=veth-${name} address=${ip_cidr} gateway=${gateway}" 2>/dev/null || echo "    (veth already exists)"
        ros "/interface/bridge/port add bridge=${bridge} interface=veth-${name}" 2>/dev/null || echo "    (port already exists)"

        echo "  Creating container ${name}..."
        ros "/container/add file=${TARBALL_DIR}/${TARBALL} interface=veth-${name} root-dir=${IMAGE_DIR}/${name} name=${name} start-on-boot=yes logging=yes dns=${DNS_SERVER} mountlists=${mprefix}.config,${mprefix}.data"

        wait_state "${name}" "S"
    done
}

cmd_start() {
    echo "==> Starting containers..."
    for inst_line in "${INSTANCES[@]}"; do
        read -r name _ _ _ _ <<< "$inst_line"
        echo "  Starting ${name}..."
        ros "/container/start [find where name=\"${name}\"]" 2>/dev/null || echo "  WARN: Could not start ${name}"
    done
}

cmd_stop() {
    echo "==> Stopping containers..."
    for inst_line in "${INSTANCES[@]}"; do
        read -r name _ _ _ _ <<< "$inst_line"
        echo "  Stopping ${name}..."
        ros "/container/stop [find where name=\"${name}\"]" 2>/dev/null || true
    done
}

cmd_status() {
    echo "==> Containers:"
    ros "/container/print" 2>/dev/null | grep -E "microdns|NAME" || echo "  None found"
    echo
    echo "==> Veths:"
    ros "/interface/veth/print" 2>/dev/null | grep -E "microdns|NAME" || echo "  None found"
}

cmd_delete() {
    echo "==> Deleting all microdns containers..."
    for inst_line in "${INSTANCES[@]}"; do
        read -r name _ _ _ _ <<< "$inst_line"
        echo "  Stopping ${name}..."
        ros "/container/stop [find where name=\"${name}\"]" 2>/dev/null || true
        wait_state "${name}" "S" 2>/dev/null || true
        echo "  Removing ${name}..."
        ros "/container/remove [find where name=\"${name}\"]" 2>/dev/null || true
        wait_state "${name}" "missing" 2>/dev/null || true
    done
}

# ── Main ───────────────────────────────────────────────────────
case "${1:-}" in
    deploy)
        cmd_setup_bridge_gt
        cmd_create_mounts
        cmd_build_upload
        cmd_upload_configs
        cmd_create_containers
        echo ""
        echo "==> Containers created. Start with: $0 start"
        ;;
    upload)
        cmd_build_upload
        cmd_upload_configs
        ;;
    start)    cmd_start ;;
    stop)     cmd_stop ;;
    delete)   cmd_delete ;;
    redeploy) cmd_delete; cmd_build_upload; cmd_upload_configs; cmd_create_containers ;;
    status)   cmd_status ;;
    *)
        echo "Usage: $(basename "$0") {deploy|upload|start|stop|delete|redeploy|status}"
        echo
        echo "  deploy   - Full setup: bridge, mounts, upload, create containers"
        echo "  upload   - Upload tarball + configs only"
        echo "  start    - Start all containers"
        echo "  stop     - Stop all containers"
        echo "  delete   - Remove all containers"
        echo "  redeploy - Delete, re-upload, re-create"
        echo "  status   - Show container status"
        exit 1
        ;;
esac
