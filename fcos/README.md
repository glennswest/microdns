# MicroDNS on Fedora CoreOS

Deploy MicroDNS as a podman container on Fedora CoreOS using Ignition.

## Prerequisites

- [butane](https://coreos.github.io/butane/) installed locally
- Fedora CoreOS ISO or PXE image
- ghcr.io/glennswest/microdns image pushed (see `scripts/build-and-push.sh`)

## Quick Start

### 1. Customize the config

Edit `microdns.toml` to match your environment:

- Set `instance.id` to a unique name
- Configure `dns.auth.zones` with your zones
- Set `api.rest.api_key` for production
- Add DHCP pools if needed

### 2. Compile Butane to Ignition

```bash
butane --strict --files-dir . microdns.bu > microdns.ign
```

The `--files-dir .` flag resolves the `local: microdns.toml` reference in the Butane config.

### 3. Install FCOS

**New install:**
```bash
coreos-installer install /dev/sda --ignition-file microdns.ign
```

**VM (e.g. libvirt):**
```bash
virt-install --name microdns \
  --ram 2048 --vcpus 2 \
  --os-variant fedora-coreos-stable \
  --import --disk size=20,backing_store=fedora-coreos.qcow2 \
  --qemu-commandline="-fw_cfg name=opt/com.coreos/config,file=$(pwd)/microdns.ign"
```

### 4. Verify

```bash
# Check service status
ssh core@<host> systemctl status microdns

# Check logs
ssh core@<host> journalctl -u microdns -f

# Test DNS
dig @<host> example.com

# Test API
curl http://<host>:8080/api/v1/zones
```

## Manual Setup (Existing FCOS)

If you already have a running FCOS host:

```bash
# Copy config
scp microdns.toml core@<host>:/etc/microdns/microdns.toml

# Create data directory
ssh core@<host> sudo mkdir -p /var/lib/microdns

# Create systemd unit
ssh core@<host> sudo tee /etc/systemd/system/microdns.service <<'EOF'
[Unit]
Description=MicroDNS
After=network-online.target
Wants=network-online.target

[Service]
Type=exec
ExecStartPre=-/usr/bin/podman pull ghcr.io/glennswest/microdns:latest
ExecStartPre=-/usr/bin/podman rm -f microdns
ExecStart=/usr/bin/podman run --rm --name microdns \
  --net=host \
  --cap-add=NET_BIND_SERVICE \
  --cap-add=NET_RAW \
  -v /etc/microdns:/etc/microdns:ro,Z \
  -v /var/lib/microdns:/data:Z \
  ghcr.io/glennswest/microdns:latest \
  --config /etc/microdns/microdns.toml
ExecStop=/usr/bin/podman stop -t 10 microdns
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

# Enable and start
ssh core@<host> sudo systemctl daemon-reload
ssh core@<host> sudo systemctl enable --now microdns
```

## Ports

| Port | Protocol | Service |
|------|----------|---------|
| 53 | UDP/TCP | Auth DNS |
| 5353 | UDP/TCP | Recursive DNS |
| 67 | UDP | DHCPv4 |
| 547 | UDP | DHCPv6 |
| 8080 | TCP | REST API |
| 50051 | TCP | gRPC |

## Updating

The service pulls the latest image on every restart:

```bash
ssh core@<host> sudo systemctl restart microdns
```

To pin a specific version, edit the service unit and replace `:latest` with a version tag.
