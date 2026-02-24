# Changelog

## [Unreleased]

### 2026-02-24
- **chore:** Add build.sh/deploy.sh for podman scratch container build + push to local registry (matches ipmiserial pattern)
- **fix:** Dedup DNS record creation — when creating a record with identical name, type, and data to an existing record, return the existing record (HTTP 200) instead of creating a duplicate. Prevents accumulation of duplicate entries from repeated mkube reconcile cycles.

### 2026-02-23
- **feat:** Add in-memory log ring buffer (1000 entries) with REST endpoint `GET /api/v1/logs?limit=100&level=info&module=dhcp`
- **feat:** Custom tracing Layer captures all log events into queryable ring buffer
- **fix:** Add 30s DHCP recv deadman timer — auto-recycles socket when stuck (veth corruption recovery)
- **fix:** Replace fatal `bind_recv_socket` crash with 5s retry loop for transient bind failures
- **fix:** Elevate DHCP activity logs (Discover/Offer/Request/ACK) from debug to info level
- **fix:** Veth corruption workaround — per-transaction bind/send/drop socket pattern
- **feat:** iPXE client detection (option 175 + user-class) with HTTP boot URL support
- **feat:** Configurable `server_ip` for siaddr/option 54 (prevents DHCP relay confusion)
- **fix:** Force broadcast flag on relay responses for proper client delivery
- **fix:** Handle SIGTERM in addition to SIGINT for container lifecycle
- **fix:** Skip raw broadcast DHCP packets (giaddr=0) — only process relay unicasts
