use axum::response::Html;

pub async fn dashboard_page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>MicroDNS</title>
<style>
@import url('https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&family=DM+Sans:wght@400;500;600;700&display=swap');

:root {
  --bg-base: #0a0e1a;
  --bg-surface: #111627;
  --bg-raised: #181d30;
  --bg-hover: #1f2640;
  --border: #252b45;
  --border-active: #3d4566;
  --text-primary: #e4e8f7;
  --text-secondary: #8b93b3;
  --text-muted: #5c6488;
  --accent: #5b8def;
  --accent-dim: #3a5ea8;
  --accent-bg: rgba(91,141,239,.1);
  --green: #4ade80;
  --green-bg: rgba(74,222,128,.12);
  --red: #f87171;
  --red-bg: rgba(248,113,113,.12);
  --amber: #fbbf24;
  --amber-bg: rgba(251,191,36,.12);
  --cyan: #22d3ee;
  --mono: 'JetBrains Mono', 'SF Mono', Monaco, Consolas, monospace;
  --sans: 'DM Sans', -apple-system, BlinkMacSystemFont, sans-serif;
}

* { margin:0; padding:0; box-sizing:border-box; }
html { font-size: 13px; }
body { font-family: var(--sans); background: var(--bg-base); color: var(--text-primary); min-height: 100vh; }

/* Scrollbar */
::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--border-active); }

/* ═══ Header ═══ */
.header {
  background: var(--bg-surface);
  padding: 0 20px;
  height: 44px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid var(--border);
  position: sticky;
  top: 0;
  z-index: 50;
}
.header-brand {
  display: flex;
  align-items: center;
  gap: 10px;
}
.header-brand svg { opacity: .7; }
.header h1 {
  font-family: var(--mono);
  font-size: 14px;
  font-weight: 700;
  letter-spacing: -.02em;
  background: linear-gradient(135deg, var(--accent), var(--cyan));
  -webkit-background-clip: text;
  -webkit-text-fill-color: transparent;
}
.ws-status {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  font-family: var(--mono);
  color: var(--text-muted);
}
.pulse {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  display: inline-block;
  position: relative;
}
.pulse.on { background: var(--green); box-shadow: 0 0 6px var(--green); }
.pulse.off { background: var(--red); }
.pulse.on::after {
  content: '';
  position: absolute;
  inset: -3px;
  border-radius: 50%;
  border: 1px solid var(--green);
  opacity: 0;
  animation: ping 2s cubic-bezier(0,.5,.5,1) infinite;
}
@keyframes ping { 0% { opacity:.6; transform:scale(.8); } 100% { opacity:0; transform:scale(1.8); } }

/* ═══ Tabs ═══ */
.tabs {
  display: flex;
  gap: 0;
  background: var(--bg-surface);
  border-bottom: 1px solid var(--border);
  padding: 0 20px;
  overflow-x: auto;
}
.tab {
  padding: 9px 16px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
  cursor: pointer;
  border-bottom: 2px solid transparent;
  transition: all .12s;
  white-space: nowrap;
  letter-spacing: .02em;
  text-transform: uppercase;
}
.tab:hover { color: var(--text-secondary); }
.tab.active { color: var(--accent); border-bottom-color: var(--accent); }
.tab .badge-count {
  display: inline-block;
  background: var(--accent-bg);
  color: var(--accent);
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 8px;
  margin-left: 4px;
  font-weight: 700;
}

/* ═══ Layout ═══ */
.content { padding: 16px 20px; display: none; }
.content.active { display: block; }
.grid { display: grid; gap: 12px; margin-bottom: 16px; }
.g2 { grid-template-columns: 1fr 1fr; }
.g3 { grid-template-columns: repeat(3, 1fr); }
.g4 { grid-template-columns: repeat(4, 1fr); }

/* ═══ Cards ═══ */
.card {
  background: var(--bg-surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 14px;
}
.card-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}
.card-title {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: .06em;
  color: var(--text-muted);
  font-weight: 600;
}

/* Stats */
.stat { text-align: center; padding: 16px 14px; }
.stat .val {
  font-family: var(--mono);
  font-size: 26px;
  font-weight: 700;
  color: var(--text-primary);
  line-height: 1;
}
.stat .lbl {
  font-size: 11px;
  color: var(--text-muted);
  margin-top: 4px;
  letter-spacing: .02em;
}

/* ═══ Tables ═══ */
table { width: 100%; border-collapse: collapse; font-size: 12px; }
th {
  text-align: left;
  padding: 6px 8px;
  color: var(--text-muted);
  border-bottom: 1px solid var(--border);
  font-weight: 600;
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: .04em;
}
td { padding: 6px 8px; border-bottom: 1px solid rgba(37,43,69,.5); }
tr:hover td { background: rgba(31,38,64,.4); }
.mono { font-family: var(--mono); font-size: 11px; }

/* ═══ Badges ═══ */
.badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 7px;
  border-radius: 3px;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: .03em;
  text-transform: uppercase;
  font-family: var(--mono);
}
.badge.ok { background: var(--green-bg); color: var(--green); }
.badge.err { background: var(--red-bg); color: var(--red); }
.badge.warn { background: var(--amber-bg); color: var(--amber); }
.badge.info { background: var(--accent-bg); color: var(--accent); }
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  display: inline-block;
  flex-shrink: 0;
}
.dot.green { background: var(--green); }
.dot.red { background: var(--red); }
.dot.amber { background: var(--amber); }
.dot.gray { background: var(--text-muted); }

/* ═══ Forms ═══ */
input, select, textarea {
  background: var(--bg-base);
  border: 1px solid var(--border);
  color: var(--text-primary);
  padding: 6px 10px;
  border-radius: 4px;
  font-size: 12px;
  font-family: var(--sans);
  transition: border-color .12s;
}
input:focus, select:focus, textarea:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 2px var(--accent-bg);
}
input::placeholder { color: var(--text-muted); }
.form-row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.form-group { display: flex; flex-direction: column; gap: 4px; }
.form-group label {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: .04em;
  color: var(--text-muted);
  font-weight: 600;
}

/* ═══ Buttons ═══ */
.btn {
  padding: 6px 14px;
  border: none;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
  transition: all .12s;
  font-family: var(--sans);
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.btn-primary { background: var(--accent); color: #fff; }
.btn-primary:hover { background: #4a7cd9; box-shadow: 0 2px 8px rgba(91,141,239,.3); }
.btn-danger { background: var(--red-bg); color: var(--red); border: 1px solid rgba(248,113,113,.2); }
.btn-danger:hover { background: rgba(248,113,113,.2); }
.btn-ghost { background: transparent; color: var(--text-secondary); border: 1px solid var(--border); }
.btn-ghost:hover { background: var(--bg-hover); border-color: var(--border-active); }
.btn-sm { padding: 3px 8px; font-size: 10px; }
.btn-xs { padding: 2px 6px; font-size: 10px; }

/* ═══ DNS Layout ═══ */
.dns-layout { display: grid; grid-template-columns: 280px 1fr; gap: 12px; min-height: 450px; }
.zone-list { background: var(--bg-surface); border: 1px solid var(--border); border-radius: 6px; overflow: hidden; }
.zone-list-header {
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.zone-item {
  padding: 7px 12px;
  cursor: pointer;
  border-bottom: 1px solid rgba(37,43,69,.4);
  font-size: 12px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  transition: background .1s;
}
.zone-item:hover { background: var(--bg-hover); }
.zone-item.selected { background: var(--accent-bg); border-left: 2px solid var(--accent); }
.zone-item .name { font-weight: 500; font-family: var(--mono); font-size: 12px; }
.zone-item .count { color: var(--text-muted); font-size: 10px; }
.records-panel { background: var(--bg-surface); border: 1px solid var(--border); border-radius: 6px; overflow: hidden; }
.records-header {
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.records-body { padding: 8px; overflow-x: auto; }
.empty-state { color: var(--text-muted); text-align: center; padding: 40px; font-size: 12px; }

/* Inline edit */
.edit-row td { background: var(--bg-base); }
.edit-row input, .edit-row select { width: 100%; padding: 4px 6px; font-size: 11px; }

/* ═══ DHCP Sections ═══ */
.dhcp-section { margin-bottom: 16px; }
.dhcp-section:last-child { margin-bottom: 0; }
.section-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 10px;
}
.section-toolbar h3 {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: .06em;
  color: var(--text-muted);
  font-weight: 600;
}

/* ═══ Modal ═══ */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 200;
  backdrop-filter: blur(4px);
}
.modal {
  background: var(--bg-raised);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 20px;
  max-width: 560px;
  width: 90%;
  max-height: 80vh;
  overflow-y: auto;
}
.modal h3 {
  font-size: 14px;
  margin-bottom: 16px;
  display: flex;
  align-items: center;
  gap: 8px;
}
.modal .form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}
.modal .form-grid .full { grid-column: 1 / -1; }
.modal-footer {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}
.modal .error-msg { color: var(--red); font-size: 11px; margin-top: 8px; }

/* ═══ Events ═══ */
.event-feed { max-height: calc(100vh - 220px); overflow-y: auto; }
.event-item {
  display: grid;
  grid-template-columns: 160px 120px 1fr;
  gap: 8px;
  padding: 5px 8px;
  border-bottom: 1px solid rgba(37,43,69,.4);
  font-size: 12px;
  align-items: center;
  animation: fadeIn .2s ease;
}
@keyframes fadeIn { from { opacity:0; transform:translateY(-4px); } to { opacity:1; transform:translateY(0); } }
.event-item .ts { color: var(--text-muted); font-family: var(--mono); font-size: 10px; }
.event-item .action { font-weight: 600; font-size: 10px; text-transform: uppercase; letter-spacing: .03em; }
.event-item .action.added { color: var(--green); }
.event-item .action.modified { color: var(--amber); }
.event-item .action.deleted { color: var(--red); }
.event-item .detail { color: var(--text-secondary); }
.event-type-badge {
  font-family: var(--mono);
  font-size: 9px;
  padding: 1px 5px;
  border-radius: 2px;
  background: var(--accent-bg);
  color: var(--accent);
  font-weight: 600;
}

/* ═══ Logs ═══ */
.log-entry {
  font-family: var(--mono);
  font-size: 11px;
  padding: 3px 0;
  border-bottom: 1px solid rgba(37,43,69,.3);
  display: grid;
  grid-template-columns: 150px 46px 110px 1fr;
  gap: 6px;
  line-height: 1.5;
}
.log-entry .ts { color: var(--text-muted); }
.log-entry .mod { color: var(--text-secondary); }
.log-entry.error .lvl { color: var(--red); font-weight: 600; }
.log-entry.warn .lvl { color: var(--amber); font-weight: 600; }
.log-entry.info .lvl { color: var(--text-secondary); }
.log-entry.debug .lvl { color: var(--text-muted); }
.log-entry .msg { word-break: break-all; }

/* ═══ Peers ═══ */
.peer-cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 12px; }
.peer-card {
  background: var(--bg-surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 14px;
}
.peer-card h3 { font-size: 13px; font-weight: 600; margin-bottom: 3px; }
.peer-card .addr { color: var(--text-muted); font-family: var(--mono); font-size: 11px; margin-bottom: 10px; }
.probe-row { display: flex; justify-content: space-between; align-items: center; padding: 4px 0; font-size: 12px; }
.probe-label { color: var(--text-secondary); }
.probe-result { display: flex; align-items: center; gap: 6px; }
.probe-latency { color: var(--text-muted); font-family: var(--mono); font-size: 11px; }

/* ═══ Responsive ═══ */
@media(max-width: 768px) {
  .dns-layout { grid-template-columns: 1fr; }
  .g4 { grid-template-columns: 1fr 1fr; }
  .g3 { grid-template-columns: 1fr; }
  .log-entry { grid-template-columns: 1fr; gap: 2px; }
  .modal .form-grid { grid-template-columns: 1fr; }
}
</style>
</head>
<body>

<div class="header">
  <div class="header-brand">
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="color:var(--accent)">
      <circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>
    </svg>
    <h1>MICRODNS</h1>
  </div>
  <div class="ws-status">
    <span class="pulse" id="ws-dot"></span>
    <span id="connection-status">connecting</span>
  </div>
</div>

<div class="tabs" id="tab-bar">
  <div class="tab active" onclick="switchTab('overview')">Overview</div>
  <div class="tab" onclick="switchTab('dns')">DNS</div>
  <div class="tab" onclick="switchTab('lb')">Load Balancer</div>
  <div class="tab" onclick="switchTab('dhcp')">DHCP</div>
  <div class="tab" onclick="switchTab('events')">Events <span class="badge-count" id="event-count" style="display:none">0</span></div>
  <div class="tab" onclick="switchTab('logs')">Logs</div>
  <div class="tab" onclick="switchTab('peers')">Peers</div>
</div>

<!-- ══════════════════════ OVERVIEW ══════════════════════ -->
<div class="content active" id="tab-overview">
  <div class="grid g3">
    <div class="card stat"><div class="val" id="zone-count">-</div><div class="lbl">Zones</div></div>
    <div class="card stat"><div class="val" id="lease-count">-</div><div class="lbl">Active Leases</div></div>
    <div class="card stat"><div class="val" id="instance-count">-</div><div class="lbl">Instances</div></div>
  </div>
  <div class="grid g2">
    <div class="card">
      <div class="card-title" style="margin-bottom:10px">Health</div>
      <table>
        <tr><td>Status</td><td id="health-status">-</td></tr>
        <tr><td>Version</td><td id="health-version">-</td></tr>
      </table>
    </div>
    <div class="card">
      <div class="card-title" style="margin-bottom:10px">Peer Connectivity</div>
      <div id="connectivity-summary"><span style="color:var(--text-muted)">Loading...</span></div>
    </div>
  </div>
  <div class="card" id="instances-card" style="display:none">
    <div class="card-title" style="margin-bottom:10px">Cluster Instances</div>
    <table>
      <thead><tr><th>Instance</th><th>Mode</th><th>Status</th><th>Leases</th></tr></thead>
      <tbody id="instances-table"></tbody>
    </table>
  </div>
  <div class="card">
    <div class="card-title" style="margin-bottom:10px">Zones</div>
    <table>
      <thead><tr><th>Name</th><th>Records</th></tr></thead>
      <tbody id="overview-zones-table"></tbody>
    </table>
  </div>
</div>

<!-- ══════════════════════ DNS ══════════════════════ -->
<div class="content" id="tab-dns">
  <div class="dns-layout">
    <div class="zone-list">
      <div class="zone-list-header">
        <span class="card-title">Zones</span>
        <button class="btn btn-primary btn-sm" onclick="showAddZone()">+ Add</button>
      </div>
      <div id="add-zone-form" style="display:none;padding:8px 12px;border-bottom:1px solid var(--border)">
        <div class="form-row" style="margin-bottom:4px">
          <input type="text" id="new-zone-name" placeholder="zone name" style="flex:1">
          <input type="number" id="new-zone-ttl" placeholder="TTL" value="300" style="width:60px">
          <button class="btn btn-primary btn-sm" onclick="createZone()">Create</button>
          <button class="btn btn-ghost btn-sm" onclick="hideAddZone()">X</button>
        </div>
        <div id="zone-error" style="color:var(--red);font-size:11px;display:none"></div>
      </div>
      <div id="zones-list-body"></div>
    </div>
    <div class="records-panel">
      <div class="records-header">
        <span class="card-title">Records <span id="records-zone-name" style="color:var(--text-primary);text-transform:none;letter-spacing:0;font-size:12px;font-weight:400"></span></span>
        <button class="btn btn-primary btn-sm" id="add-record-btn" style="display:none" onclick="showAddRecord()">+ Add</button>
      </div>
      <div class="records-body" id="records-body">
        <div class="empty-state">Select a zone to view records</div>
      </div>
    </div>
  </div>
</div>

<!-- ══════════════════════ LOAD BALANCER ══════════════════════ -->
<div class="content" id="tab-lb">
  <div class="section-toolbar">
    <h3>Health-Checked Records</h3>
    <button class="btn btn-ghost btn-sm" onclick="loadLB()">Refresh</button>
  </div>
  <div class="grid g4" style="margin-bottom:12px">
    <div class="card stat"><div class="val" id="lb-total">-</div><div class="lbl">Monitored</div></div>
    <div class="card stat"><div class="val" id="lb-healthy" style="color:var(--green)">-</div><div class="lbl">Healthy</div></div>
    <div class="card stat"><div class="val" id="lb-unhealthy" style="color:var(--red)">-</div><div class="lbl">Unhealthy</div></div>
    <div class="card stat"><div class="val" id="lb-groups">-</div><div class="lbl">Failover Groups</div></div>
  </div>
  <div class="card" id="lb-groups-card" style="display:none;margin-bottom:12px">
    <div class="card-title" style="margin-bottom:10px">Failover Groups</div>
    <div id="lb-groups-body"></div>
  </div>
  <div class="card">
    <div class="card-title" style="margin-bottom:10px">All Records</div>
    <table>
      <thead><tr><th>Zone</th><th>Name</th><th>Type</th><th>Target</th><th>Probe</th><th>Interval</th><th>Thresholds</th><th>Status</th></tr></thead>
      <tbody id="lb-records-table"></tbody>
    </table>
  </div>
</div>

<!-- ══════════════════════ DHCP ══════════════════════ -->
<div class="content" id="tab-dhcp">
  <div class="grid g4">
    <div class="card stat"><div class="val" id="dhcp-pool-count">-</div><div class="lbl">Pools</div></div>
    <div class="card stat"><div class="val" id="dhcp-res-count">-</div><div class="lbl">Reservations</div></div>
    <div class="card stat"><div class="val" id="dhcp-active">-</div><div class="lbl">Active Leases</div></div>
    <div class="card stat"><div class="val" id="dhcp-enabled">-</div><div class="lbl">Status</div></div>
  </div>

  <!-- Pools -->
  <div class="dhcp-section">
    <div class="section-toolbar">
      <h3>Pools</h3>
      <button class="btn btn-primary btn-sm" onclick="showPoolModal()">+ Add Pool</button>
    </div>
    <div class="card" style="padding:0;overflow:hidden">
      <table>
        <thead><tr><th>Name</th><th>Range</th><th>Subnet</th><th>Gateway</th><th>Domain</th><th>Lease</th><th>PXE</th><th style="width:70px"></th></tr></thead>
        <tbody id="dhcp-pools-table"></tbody>
      </table>
    </div>
  </div>

  <!-- Reservations -->
  <div class="dhcp-section">
    <div class="section-toolbar">
      <h3>Reservations</h3>
      <div class="form-row">
        <input type="text" id="res-search" placeholder="Search MAC, IP, hostname..." style="width:220px" oninput="filterReservations()">
        <button class="btn btn-primary btn-sm" onclick="showResModal()">+ Add Reservation</button>
      </div>
    </div>
    <div class="card" style="padding:0;overflow:hidden">
      <table>
        <thead><tr><th>MAC</th><th>IP</th><th>Hostname</th><th>Gateway</th><th>DNS</th><th>PXE</th><th style="width:70px"></th></tr></thead>
        <tbody id="dhcp-res-table"></tbody>
      </table>
    </div>
  </div>

  <!-- Active Leases -->
  <div class="dhcp-section">
    <div class="section-toolbar">
      <h3>Active Leases</h3>
      <input type="text" id="lease-search" placeholder="Search..." style="width:220px" oninput="filterLeases()">
    </div>
    <div class="card" style="padding:0;overflow:hidden">
      <table>
        <thead><tr><th>IP Address</th><th>MAC Address</th><th>Hostname</th><th>Lease Start</th><th>Expires</th><th>State</th></tr></thead>
        <tbody id="dhcp-leases-table"></tbody>
      </table>
    </div>
  </div>
</div>

<!-- ══════════════════════ EVENTS ══════════════════════ -->
<div class="content" id="tab-events">
  <div class="section-toolbar">
    <h3>Real-time Events</h3>
    <div class="form-row">
      <select id="event-filter" onchange="renderEvents()">
        <option value="">All Events</option>
        <option value="DhcpPoolChanged">DHCP Pools</option>
        <option value="DhcpReservationChanged">DHCP Reservations</option>
        <option value="DnsForwarderChanged">DNS Forwarders</option>
        <option value="LeaseChanged">Leases</option>
        <option value="ZoneChanged">Zones</option>
        <option value="RecordChanged">Records</option>
      </select>
      <button class="btn btn-ghost btn-sm" onclick="clearEvents()">Clear</button>
      <label style="font-size:11px;display:flex;align-items:center;gap:5px;cursor:pointer;color:var(--text-secondary)">
        <input type="checkbox" id="event-auto-scroll" checked> Auto-scroll
      </label>
    </div>
  </div>
  <div class="card" style="padding:8px">
    <div class="event-feed" id="event-feed">
      <div class="empty-state">Waiting for events...</div>
    </div>
  </div>
</div>

<!-- ══════════════════════ LOGS ══════════════════════ -->
<div class="content" id="tab-logs">
  <div class="form-row" style="margin-bottom:12px">
    <select id="log-level" onchange="loadLogs()">
      <option value="">All Levels</option>
      <option value="error">ERROR</option>
      <option value="warn">WARN</option>
      <option value="info" selected>INFO</option>
      <option value="debug">DEBUG</option>
    </select>
    <input type="text" id="log-module" placeholder="Module filter..." style="width:180px" onchange="loadLogs()">
    <label style="font-size:11px;display:flex;align-items:center;gap:5px;cursor:pointer;color:var(--text-secondary)">
      <input type="checkbox" id="log-auto" checked onchange="toggleLogAuto()"> Auto-refresh
    </label>
    <button class="btn btn-ghost btn-sm" onclick="loadLogs()">Refresh</button>
  </div>
  <div class="card" style="max-height:calc(100vh - 200px);overflow-y:auto">
    <div id="log-entries"></div>
  </div>
</div>

<!-- ══════════════════════ PEERS ══════════════════════ -->
<div class="content" id="tab-peers">
  <div class="section-toolbar">
    <h3>Peer Connectivity Probes</h3>
    <button class="btn btn-ghost btn-sm" onclick="loadPeers()">Refresh</button>
  </div>
  <div class="peer-cards" id="peer-cards"></div>
</div>

<!-- ══════════════════════ MODALS ══════════════════════ -->

<!-- Confirm Dialog -->
<div class="modal-overlay" id="confirm-dialog" style="display:none">
  <div class="modal" style="max-width:380px">
    <h3 id="confirm-title">Confirm</h3>
    <p style="color:var(--text-secondary);font-size:12px;margin-bottom:16px" id="confirm-msg"></p>
    <div class="modal-footer" style="border:none;margin:0;padding:0">
      <button class="btn btn-ghost" onclick="closeConfirm()">Cancel</button>
      <button class="btn btn-danger" id="confirm-ok" onclick="doConfirm()">Delete</button>
    </div>
  </div>
</div>

<!-- Pool Modal -->
<div class="modal-overlay" id="pool-modal" style="display:none">
  <div class="modal">
    <h3 id="pool-modal-title">Add Pool</h3>
    <div class="form-grid">
      <div class="form-group full">
        <label>Pool Name</label>
        <input type="text" id="pm-name" placeholder="e.g. g10 DHCP Pool">
      </div>
      <div class="form-group">
        <label>Range Start</label>
        <input type="text" id="pm-range-start" placeholder="192.168.10.100">
      </div>
      <div class="form-group">
        <label>Range End</label>
        <input type="text" id="pm-range-end" placeholder="192.168.10.200">
      </div>
      <div class="form-group">
        <label>Subnet</label>
        <input type="text" id="pm-subnet" placeholder="192.168.10.0/24">
      </div>
      <div class="form-group">
        <label>Gateway</label>
        <input type="text" id="pm-gateway" placeholder="192.168.10.1">
      </div>
      <div class="form-group">
        <label>DNS Servers (comma-sep)</label>
        <input type="text" id="pm-dns" placeholder="192.168.10.252">
      </div>
      <div class="form-group">
        <label>Domain</label>
        <input type="text" id="pm-domain" placeholder="g10.lo">
      </div>
      <div class="form-group">
        <label>Lease Time (seconds)</label>
        <input type="number" id="pm-lease" value="3600">
      </div>
      <div class="form-group">
        <label>MTU</label>
        <input type="number" id="pm-mtu" placeholder="1500">
      </div>
      <div class="form-group full">
        <label>NTP Servers (comma-sep)</label>
        <input type="text" id="pm-ntp" placeholder="">
      </div>
      <div class="form-group">
        <label>Next Server (PXE TFTP)</label>
        <input type="text" id="pm-next-server" placeholder="">
      </div>
      <div class="form-group">
        <label>Boot File (BIOS)</label>
        <input type="text" id="pm-boot-file" placeholder="undionly.kpxe">
      </div>
      <div class="form-group">
        <label>Boot File (EFI)</label>
        <input type="text" id="pm-boot-file-efi" placeholder="ipxe.efi">
      </div>
      <div class="form-group">
        <label>iPXE Boot URL</label>
        <input type="text" id="pm-ipxe-url" placeholder="">
      </div>
    </div>
    <div id="pool-modal-error" class="error-msg" style="display:none"></div>
    <div class="modal-footer">
      <button class="btn btn-ghost" onclick="closePoolModal()">Cancel</button>
      <button class="btn btn-primary" id="pool-modal-submit" onclick="submitPool()">Create</button>
    </div>
  </div>
</div>

<!-- Reservation Modal -->
<div class="modal-overlay" id="res-modal" style="display:none">
  <div class="modal">
    <h3 id="res-modal-title">Add Reservation</h3>
    <div class="form-grid">
      <div class="form-group">
        <label>MAC Address</label>
        <input type="text" id="rm-mac" placeholder="aa:bb:cc:dd:ee:ff">
      </div>
      <div class="form-group">
        <label>IP Address</label>
        <input type="text" id="rm-ip" placeholder="192.168.10.10">
      </div>
      <div class="form-group">
        <label>Hostname</label>
        <input type="text" id="rm-hostname" placeholder="server1">
      </div>
      <div class="form-group">
        <label>Domain</label>
        <input type="text" id="rm-domain" placeholder="">
      </div>
      <div class="form-group">
        <label>Gateway</label>
        <input type="text" id="rm-gateway" placeholder="">
      </div>
      <div class="form-group">
        <label>DNS Servers (comma-sep)</label>
        <input type="text" id="rm-dns" placeholder="">
      </div>
      <div class="form-group">
        <label>Lease Time (seconds)</label>
        <input type="number" id="rm-lease" placeholder="">
      </div>
      <div class="form-group">
        <label>MTU</label>
        <input type="number" id="rm-mtu" placeholder="">
      </div>
      <div class="form-group full">
        <label>NTP Servers (comma-sep)</label>
        <input type="text" id="rm-ntp" placeholder="">
      </div>
      <div class="form-group">
        <label>Next Server (PXE TFTP)</label>
        <input type="text" id="rm-next-server" placeholder="">
      </div>
      <div class="form-group">
        <label>Boot File (BIOS)</label>
        <input type="text" id="rm-boot-file" placeholder="">
      </div>
      <div class="form-group">
        <label>Boot File (EFI)</label>
        <input type="text" id="rm-boot-file-efi" placeholder="">
      </div>
      <div class="form-group full">
        <label>iPXE Boot URL</label>
        <input type="text" id="rm-ipxe-url" placeholder="">
      </div>
    </div>
    <div id="res-modal-error" class="error-msg" style="display:none"></div>
    <div class="modal-footer">
      <button class="btn btn-ghost" onclick="closeResModal()">Cancel</button>
      <button class="btn btn-primary" id="res-modal-submit" onclick="submitRes()">Create</button>
    </div>
  </div>
</div>

<script>
const API = `http://${location.hostname}:8080/api/v1`;
let ws, wsData = {zones:[], leases:[], instances:[]};
let selectedZoneId = null, selectedZoneName = '';
let allLeases = [], allPools = [], allReservations = [];
let events = [];
let intervals = {};
let confirmCb = null;
let editPoolId = null, editResMac = null;

// ─── Helpers ───

function esc(s) { if(s==null) return ''; const d=document.createElement('div'); d.textContent=String(s); return d.innerHTML; }
function orDash(v) { return v || '-'; }
function csvToArr(s) { return s ? s.split(',').map(x=>x.trim()).filter(Boolean) : []; }
function arrToCsv(a) { return (a||[]).join(', '); }

async function apiFetch(path) {
  const r = await fetch(API + path);
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}
async function apiPost(path, body) {
  const r = await fetch(API + path, {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
  if (!r.ok) { const t = await r.text(); throw new Error(t || r.statusText); }
  return r.json();
}
async function apiPatch(path, body) {
  const r = await fetch(API + path, {method:'PATCH', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
  if (!r.ok) { const t = await r.text(); throw new Error(t || r.statusText); }
  return r.json();
}
async function apiPut(path, body) {
  const r = await fetch(API + path, {method:'PUT', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
  if (!r.ok) { const t = await r.text(); throw new Error(t || r.statusText); }
  return r.json();
}
async function apiDelete(path) {
  const r = await fetch(API + path, {method:'DELETE'});
  if (r.status !== 204 && !r.ok) throw new Error(`${r.status} ${r.statusText}`);
}

function relTime(iso) {
  const d = new Date(iso), now = Date.now(), diff = d - now;
  if (isNaN(d)) return iso;
  if (diff < 0) return 'expired';
  const s = Math.floor(diff/1000), m = Math.floor(s/60), h = Math.floor(m/60);
  if (m < 1) return `${s}s`;
  if (h < 1) return `${m}m`;
  return `${h}h ${m%60}m`;
}

function fmtRecordData(data) {
  if (!data) return '-';
  switch(data.type) {
    case 'MX': return `${data.data.preference} ${data.data.exchange}`;
    case 'SRV': return `${data.data.priority} ${data.data.weight} ${data.data.port} ${data.data.target}`;
    case 'CAA': return `${data.data.flags} ${data.data.tag} "${data.data.value}"`;
    default: return typeof data.data === 'string' ? data.data : JSON.stringify(data.data);
  }
}

function showConfirm(title, msg, cb) {
  document.getElementById('confirm-title').textContent = title;
  document.getElementById('confirm-msg').textContent = msg;
  confirmCb = cb;
  document.getElementById('confirm-dialog').style.display = 'flex';
}
function closeConfirm() { document.getElementById('confirm-dialog').style.display = 'none'; confirmCb = null; }
function doConfirm() { if(confirmCb) confirmCb(); closeConfirm(); }

// ─── Tab Management ───

function switchTab(name) {
  const tabNames = ['overview','dns','lb','dhcp','events','logs','peers'];
  document.querySelectorAll('.tab').forEach((t,i) => t.classList.toggle('active', tabNames[i] === name));
  document.querySelectorAll('.content').forEach(c => c.classList.remove('active'));
  document.getElementById('tab-' + name).classList.add('active');

  Object.values(intervals).forEach(i => clearInterval(i));
  intervals = {};

  switch(name) {
    case 'overview': startOverview(); break;
    case 'dns': break;
    case 'lb': loadLB(); intervals.lb = setInterval(loadLB, 10000); break;
    case 'dhcp': loadDhcp(); intervals.dhcp = setInterval(loadDhcp, 5000); break;
    case 'events': renderEvents(); break;
    case 'logs': loadLogs(); if(document.getElementById('log-auto').checked) intervals.logs = setInterval(loadLogs, 3000); break;
    case 'peers': loadPeers(); intervals.peers = setInterval(loadPeers, 10000); break;
  }
}

// ─── WebSocket ───

function connect() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  ws = new WebSocket(proto + '//' + location.host + '/ws');
  const dot = document.getElementById('ws-dot');
  const status = document.getElementById('connection-status');

  ws.onopen = () => { dot.className='pulse on'; status.textContent='connected'; };
  ws.onclose = () => { dot.className='pulse off'; status.textContent='disconnected'; setTimeout(connect,3000); };
  ws.onerror = () => { dot.className='pulse off'; status.textContent='error'; };

  ws.onmessage = (evt) => {
    try {
      const msg = JSON.parse(evt.data);
      if (msg.msg_type === 'snapshot') {
        wsData = msg;
        updateFromSnapshot();
      } else if (msg.msg_type === 'event') {
        handleEvent(msg.event);
      }
    } catch(e) {}
  };
}

function updateFromSnapshot() {
  document.getElementById('zone-count').textContent = wsData.zones.length;
  document.getElementById('lease-count').textContent = wsData.leases.length;
  document.getElementById('instance-count').textContent = wsData.instances.length;

  document.getElementById('overview-zones-table').innerHTML = wsData.zones
    .map(z => `<tr><td><span class="mono">${esc(z.name)}</span></td><td>${z.record_count}</td></tr>`).join('');

  if (wsData.instances.length > 0) {
    document.getElementById('instances-card').style.display = '';
    document.getElementById('instances-table').innerHTML = wsData.instances
      .map(i => `<tr><td>${esc(i.instance_id)}</td><td>${esc(i.mode)}</td><td><span class="badge ${i.healthy?'ok':'err'}">${i.healthy?'Healthy':'Down'}</span></td><td>${i.active_leases}</td></tr>`).join('');
  } else {
    document.getElementById('instances-card').style.display = 'none';
  }

  renderZoneList();
}

function handleEvent(event) {
  event._time = new Date().toISOString();
  events.unshift(event);
  if (events.length > 500) events.length = 500;

  // Update event count badge
  const badge = document.getElementById('event-count');
  badge.style.display = '';
  badge.textContent = events.length;

  // Re-render if events tab is active
  if (document.getElementById('tab-events').classList.contains('active')) {
    renderEvents();
  }
}

// ─── Overview ───

function startOverview() {
  loadHealth();
  loadConnectivitySummary();
  intervals.health = setInterval(loadHealth, 10000);
  intervals.conn = setInterval(loadConnectivitySummary, 10000);
}

async function loadHealth() {
  try {
    const h = await apiFetch('/health');
    document.getElementById('health-status').innerHTML = `<span class="badge ok">${esc(h.status)}</span>`;
    document.getElementById('health-version').textContent = h.version;
  } catch(e) {
    document.getElementById('health-status').innerHTML = '<span class="badge err">error</span>';
  }
}

async function loadConnectivitySummary() {
  try {
    const c = await apiFetch('/connectivity');
    if (!c.peers || c.peers.length === 0) {
      document.getElementById('connectivity-summary').innerHTML = '<span style="color:var(--text-muted)">No peers configured</span>';
      return;
    }
    document.getElementById('connectivity-summary').innerHTML = c.peers.map(p => {
      const probes = [{name:'UDP',r:p.dns_udp},{name:'TCP',r:p.dns_tcp},{name:'HTTP',r:p.http}];
      const dots = probes.map(pr =>
        `<span class="dot ${pr.r.ok?'green':'red'}" title="${pr.name}: ${pr.r.ok ? pr.r.latency_ms.toFixed(1)+'ms' : pr.r.error}"></span>`
      ).join('');
      return `<div style="display:flex;align-items:center;gap:8px;padding:3px 0;font-size:12px">
        <span class="mono" style="min-width:80px">${esc(p.id)}</span>${dots}
        <span style="color:var(--text-muted);font-size:10px">${p.dns_udp.ok ? p.dns_udp.latency_ms.toFixed(1)+'ms' : ''}</span>
      </div>`;
    }).join('');
  } catch(e) {
    document.getElementById('connectivity-summary').innerHTML = '<span style="color:var(--text-muted)">Unavailable</span>';
  }
}

// ─── DNS Tab ───

function renderZoneList() {
  const body = document.getElementById('zones-list-body');
  body.innerHTML = wsData.zones.map(z =>
    `<div class="zone-item ${z.id===selectedZoneId?'selected':''}" onclick="selectZone('${z.id}','${esc(z.name)}')">
      <span class="name">${esc(z.name)}</span>
      <span style="display:flex;align-items:center;gap:6px">
        <span class="count">${z.record_count}</span>
        <button class="btn btn-danger btn-xs" onclick="event.stopPropagation();confirmDeleteZone('${z.id}','${esc(z.name)}')" title="Delete">&times;</button>
      </span>
    </div>`
  ).join('');
}

function showAddZone() { document.getElementById('add-zone-form').style.display=''; document.getElementById('new-zone-name').focus(); }
function hideAddZone() { document.getElementById('add-zone-form').style.display='none'; document.getElementById('zone-error').style.display='none'; }

async function createZone() {
  const name = document.getElementById('new-zone-name').value.trim();
  const ttl = parseInt(document.getElementById('new-zone-ttl').value) || 300;
  if (!name) return;
  try {
    await apiPost('/zones', {name, default_ttl: ttl});
    hideAddZone();
    document.getElementById('new-zone-name').value = '';
  } catch(e) {
    const el = document.getElementById('zone-error');
    el.textContent = e.message; el.style.display = '';
  }
}

function confirmDeleteZone(id, name) {
  showConfirm('Delete Zone', `Delete zone "${name}" and all its records?`, async () => {
    try { await apiDelete('/zones/' + id); if (selectedZoneId===id) { selectedZoneId=null; renderRecordsEmpty(); } } catch(e) { alert('Failed: '+e.message); }
  });
}

async function selectZone(id, name) {
  selectedZoneId = id; selectedZoneName = name;
  renderZoneList();
  document.getElementById('records-zone-name').textContent = '— ' + name;
  document.getElementById('add-record-btn').style.display = '';
  await loadRecords();
}

async function loadRecords() {
  if (!selectedZoneId) return;
  try {
    const recs = await apiFetch(`/zones/${selectedZoneId}/records?limit=500`);
    renderRecords(recs);
  } catch(e) {
    document.getElementById('records-body').innerHTML = `<div class="empty-state">Error: ${esc(e.message)}</div>`;
  }
}

function renderRecordsEmpty() {
  document.getElementById('records-zone-name').textContent = '';
  document.getElementById('add-record-btn').style.display = 'none';
  document.getElementById('records-body').innerHTML = '<div class="empty-state">Select a zone to view records</div>';
}

function renderRecords(recs) {
  if (recs.length === 0) {
    document.getElementById('records-body').innerHTML = '<div class="empty-state">No records</div>';
    return;
  }
  recs.sort((a,b) => {
    const order = {SOA:0,NS:1,A:2,AAAA:3,CNAME:4,MX:5,TXT:6,SRV:7,PTR:8,CAA:9};
    const oa = order[a.type]??10, ob = order[b.type]??10;
    if (oa !== ob) return oa - ob;
    return (a.name||'').localeCompare(b.name||'');
  });

  let html = `<table><thead><tr><th>Name</th><th>Type</th><th>Data</th><th>TTL</th><th>On</th><th style="width:70px"></th></tr></thead><tbody>`;
  html += `<tr id="add-record-row" style="display:none">${addRecordCells()}</tr>`;
  for (const r of recs) {
    html += `<tr id="rec-${r.id}">
      <td class="mono">${esc(r.name)}</td>
      <td><span class="badge info">${esc(r.type)}</span></td>
      <td class="mono">${esc(fmtRecordData(r.data))}</td>
      <td>${r.ttl}</td>
      <td><span class="dot ${r.enabled?'green':'red'}"></span></td>
      <td>
        <button class="btn btn-ghost btn-xs" onclick="startEditRecord('${r.id}')" title="Edit">&#9998;</button>
        <button class="btn btn-danger btn-xs" onclick="confirmDeleteRecord('${r.id}','${esc(r.name)}','${esc(r.type)}')" title="Delete">&times;</button>
      </td>
    </tr>`;
  }
  html += '</tbody></table>';
  document.getElementById('records-body').innerHTML = html;
  window._records = {};
  recs.forEach(r => window._records[r.id] = r);
}

function addRecordCells() {
  return `<td><input type="text" id="nr-name" placeholder="name" style="width:100%"></td>
    <td><select id="nr-type" onchange="updateRecordDataFields()" style="width:100%">
      <option>A</option><option>AAAA</option><option>CNAME</option><option>MX</option>
      <option>NS</option><option>PTR</option><option>SRV</option><option>TXT</option><option>CAA</option>
    </select></td>
    <td id="nr-data-cell"><input type="text" id="nr-data" placeholder="value" style="width:100%"></td>
    <td><input type="number" id="nr-ttl" value="300" style="width:55px"></td>
    <td></td>
    <td>
      <button class="btn btn-primary btn-xs" onclick="createRecord()">Add</button>
      <button class="btn btn-ghost btn-xs" onclick="hideAddRecord()">X</button>
    </td>`;
}

function showAddRecord() { document.getElementById('add-record-row').style.display=''; updateRecordDataFields(); const n=document.getElementById('nr-name'); if(n) n.focus(); }
function hideAddRecord() { document.getElementById('add-record-row').style.display='none'; }

function updateRecordDataFields() {
  const type = document.getElementById('nr-type').value;
  const cell = document.getElementById('nr-data-cell');
  switch(type) {
    case 'MX':
      cell.innerHTML = `<div style="display:flex;gap:4px"><input type="number" id="nr-mx-pref" placeholder="pref" value="10" style="width:45px"><input type="text" id="nr-mx-exch" placeholder="exchange" style="flex:1"></div>`;
      break;
    case 'SRV':
      cell.innerHTML = `<div style="display:flex;gap:4px"><input type="number" id="nr-srv-pri" placeholder="pri" value="10" style="width:40px"><input type="number" id="nr-srv-w" placeholder="wt" value="0" style="width:35px"><input type="number" id="nr-srv-port" placeholder="port" style="width:45px"><input type="text" id="nr-srv-target" placeholder="target" style="flex:1"></div>`;
      break;
    case 'CAA':
      cell.innerHTML = `<div style="display:flex;gap:4px"><input type="number" id="nr-caa-flags" value="0" style="width:35px"><select id="nr-caa-tag" style="width:75px"><option>issue</option><option>issuewild</option><option>iodef</option></select><input type="text" id="nr-caa-val" placeholder="value" style="flex:1"></div>`;
      break;
    default:
      cell.innerHTML = `<input type="text" id="nr-data" placeholder="value" style="width:100%">`;
  }
}

function buildRecordData(type, prefix) {
  prefix = prefix || 'nr';
  switch(type) {
    case 'A': case 'AAAA': case 'CNAME': case 'NS': case 'PTR': case 'TXT':
      return {type, data: document.getElementById(prefix+'-data').value};
    case 'MX':
      return {type:'MX', data:{preference:parseInt(document.getElementById(prefix+'-mx-pref').value)||10, exchange:document.getElementById(prefix+'-mx-exch').value}};
    case 'SRV':
      return {type:'SRV', data:{priority:parseInt(document.getElementById(prefix+'-srv-pri').value)||10, weight:parseInt(document.getElementById(prefix+'-srv-w').value)||0, port:parseInt(document.getElementById(prefix+'-srv-port').value)||0, target:document.getElementById(prefix+'-srv-target').value}};
    case 'CAA':
      return {type:'CAA', data:{flags:parseInt(document.getElementById(prefix+'-caa-flags').value)||0, tag:document.getElementById(prefix+'-caa-tag').value, value:document.getElementById(prefix+'-caa-val').value}};
  }
}

async function createRecord() {
  const name = document.getElementById('nr-name').value.trim();
  const type = document.getElementById('nr-type').value;
  const ttl = parseInt(document.getElementById('nr-ttl').value) || 300;
  const data = buildRecordData(type);
  if (!name || !data) return;
  try {
    await apiPost(`/zones/${selectedZoneId}/records`, {name, ttl, data, enabled:true});
    hideAddRecord();
    await loadRecords();
  } catch(e) { alert('Failed: ' + e.message); }
}

function confirmDeleteRecord(id, name, type) {
  showConfirm('Delete Record', `Delete ${type} record "${name}"?`, async () => {
    try { await apiDelete(`/zones/${selectedZoneId}/records/${id}`); await loadRecords(); } catch(e) { alert('Failed: ' + e.message); }
  });
}

function startEditRecord(id) {
  const r = window._records[id];
  if (!r) return;
  const row = document.getElementById('rec-' + id);
  if (!row) return;
  let dataCell;
  switch(r.type) {
    case 'MX':
      dataCell = `<div style="display:flex;gap:4px"><input type="number" id="er-mx-pref" value="${r.data.data.preference}" style="width:45px"><input type="text" id="er-mx-exch" value="${esc(r.data.data.exchange)}" style="flex:1"></div>`; break;
    case 'SRV':
      dataCell = `<div style="display:flex;gap:4px"><input type="number" id="er-srv-pri" value="${r.data.data.priority}" style="width:40px"><input type="number" id="er-srv-w" value="${r.data.data.weight}" style="width:35px"><input type="number" id="er-srv-port" value="${r.data.data.port}" style="width:45px"><input type="text" id="er-srv-target" value="${esc(r.data.data.target)}" style="flex:1"></div>`; break;
    case 'CAA':
      dataCell = `<div style="display:flex;gap:4px"><input type="number" id="er-caa-flags" value="${r.data.data.flags}" style="width:35px"><select id="er-caa-tag" style="width:75px"><option ${r.data.data.tag==='issue'?'selected':''}>issue</option><option ${r.data.data.tag==='issuewild'?'selected':''}>issuewild</option><option ${r.data.data.tag==='iodef'?'selected':''}>iodef</option></select><input type="text" id="er-caa-val" value="${esc(r.data.data.value)}" style="flex:1"></div>`; break;
    default:
      dataCell = `<input type="text" id="er-data" value="${esc(typeof r.data.data==='string'?r.data.data:JSON.stringify(r.data.data))}" style="width:100%">`;
  }
  row.className = 'edit-row';
  row.innerHTML = `
    <td><input type="text" id="er-name" value="${esc(r.name)}" style="width:100%"></td>
    <td><span class="badge info">${esc(r.type)}</span></td>
    <td>${dataCell}</td>
    <td><input type="number" id="er-ttl" value="${r.ttl}" style="width:55px"></td>
    <td><select id="er-enabled"><option value="true" ${r.enabled?'selected':''}>On</option><option value="false" ${!r.enabled?'selected':''}>Off</option></select></td>
    <td>
      <button class="btn btn-primary btn-xs" onclick="saveEditRecord('${r.id}','${esc(r.type)}')">Save</button>
      <button class="btn btn-ghost btn-xs" onclick="loadRecords()">X</button>
    </td>`;
}

async function saveEditRecord(id, type) {
  const name = document.getElementById('er-name').value.trim();
  const ttl = parseInt(document.getElementById('er-ttl').value) || 300;
  const enabled = document.getElementById('er-enabled').value === 'true';
  const data = buildRecordData(type, 'er');
  try { await apiPut(`/zones/${selectedZoneId}/records/${id}`, {name, ttl, data, enabled}); await loadRecords(); }
  catch(e) { alert('Failed: ' + e.message); }
}

// ─── Load Balancer ───

async function loadLB() {
  try {
    const zones = wsData.zones || [];
    const allRecs = [];
    await Promise.all(zones.map(async z => {
      try {
        const recs = await apiFetch(`/zones/${z.id}/records?limit=500`);
        recs.forEach(r => { r._zoneName = z.name; r._zoneId = z.id; });
        allRecs.push(...recs);
      } catch(e) {}
    }));

    const hcRecs = allRecs.filter(r => r.health_check);
    const healthy = hcRecs.filter(r => r.enabled).length;
    const unhealthy = hcRecs.length - healthy;

    const groupMap = {};
    hcRecs.forEach(r => {
      const key = `${r._zoneId}|${r.name}|${r.type}`;
      if (!groupMap[key]) groupMap[key] = {zone:r._zoneName, name:r.name, type:r.type, records:[]};
      groupMap[key].records.push(r);
    });
    const groups = Object.values(groupMap).filter(g => g.records.length >= 2);

    document.getElementById('lb-total').textContent = hcRecs.length;
    document.getElementById('lb-healthy').textContent = healthy;
    document.getElementById('lb-unhealthy').textContent = unhealthy;
    document.getElementById('lb-groups').textContent = groups.length;

    if (groups.length > 0) {
      document.getElementById('lb-groups-card').style.display = '';
      document.getElementById('lb-groups-body').innerHTML = groups.map(g => {
        const allDown = g.records.every(r => !r.enabled);
        return `<div style="margin-bottom:10px;padding:8px;background:var(--bg-base);border-radius:4px">
          <div style="display:flex;align-items:center;gap:6px;margin-bottom:4px">
            <span class="mono" style="font-weight:600">${esc(g.name)}.${esc(g.zone)}</span>
            <span class="badge info">${esc(g.type)}</span>
            ${allDown ? '<span class="badge err">FAILSAFE</span>' : ''}
          </div>
          <div style="display:flex;gap:10px;flex-wrap:wrap">${g.records.map(r =>
            `<div style="display:flex;align-items:center;gap:4px;font-size:11px">
              <span class="dot ${r.enabled?'green':'red'}"></span>
              <span class="mono">${esc(fmtRecordData(r.data))}</span>
            </div>`
          ).join('')}</div>
        </div>`;
      }).join('');
    } else {
      document.getElementById('lb-groups-card').style.display = 'none';
    }

    document.getElementById('lb-records-table').innerHTML = hcRecs.map(r => {
      const hc = r.health_check;
      return `<tr>
        <td class="mono">${esc(r._zoneName)}</td>
        <td class="mono">${esc(r.name)}</td>
        <td><span class="badge info">${esc(r.type)}</span></td>
        <td class="mono">${esc(fmtRecordData(r.data))}</td>
        <td>${esc(hc.probe_type)}</td>
        <td>${hc.interval_secs}s</td>
        <td style="font-size:10px;color:var(--text-muted)">${hc.healthy_threshold}/${hc.unhealthy_threshold}</td>
        <td><span class="badge ${r.enabled?'ok':'err'}">${r.enabled?'Healthy':'Down'}</span></td>
      </tr>`;
    }).join('') || '<tr><td colspan="8" style="color:var(--text-muted)">No health-checked records</td></tr>';
  } catch(e) {
    document.getElementById('lb-records-table').innerHTML = `<tr><td colspan="8" style="color:var(--text-muted)">Error: ${esc(e.message)}</td></tr>`;
  }
}

// ─── DHCP Tab ───

async function loadDhcp() {
  try {
    const [pools, reservations, leases, status] = await Promise.all([
      apiFetch('/dhcp/pools'),
      apiFetch('/dhcp/reservations'),
      apiFetch('/leases?limit=500'),
      apiFetch('/dhcp/status').catch(() => ({enabled:false,interface:'',reservation_count:0,active_lease_count:0}))
    ]);

    allPools = pools;
    allReservations = reservations;
    allLeases = leases;

    document.getElementById('dhcp-pool-count').textContent = pools.length;
    document.getElementById('dhcp-res-count').textContent = reservations.length;
    document.getElementById('dhcp-active').textContent = leases.length;
    document.getElementById('dhcp-enabled').textContent = status.enabled ? 'Enabled' : 'Disabled';

    // Pools table
    document.getElementById('dhcp-pools-table').innerHTML = pools.map(p =>
      `<tr>
        <td>${esc(p.name)}</td>
        <td class="mono">${esc(p.range_start)} — ${esc(p.range_end)}</td>
        <td class="mono">${esc(p.subnet)}</td>
        <td class="mono">${esc(p.gateway)}</td>
        <td>${esc(p.domain)}</td>
        <td>${p.lease_time_secs}s</td>
        <td>${p.next_server ? '<span class="badge ok">PXE</span>' : ''}</td>
        <td>
          <button class="btn btn-ghost btn-xs" onclick="editPool('${esc(p.id)}')" title="Edit">&#9998;</button>
          <button class="btn btn-danger btn-xs" onclick="confirmDeletePool('${esc(p.id)}','${esc(p.name)}')" title="Delete">&times;</button>
        </td>
      </tr>`
    ).join('') || '<tr><td colspan="8" style="color:var(--text-muted)">No pools configured</td></tr>';

    filterReservations();
    filterLeases();
  } catch(e) {}
}

function filterReservations() {
  const q = (document.getElementById('res-search').value || '').toLowerCase();
  const filtered = q ? allReservations.filter(r =>
    (r.mac||'').toLowerCase().includes(q) ||
    (r.ip||'').toLowerCase().includes(q) ||
    (r.hostname||'').toLowerCase().includes(q)
  ) : allReservations;

  document.getElementById('dhcp-res-table').innerHTML = filtered.map(r =>
    `<tr>
      <td class="mono">${esc(r.mac)}</td>
      <td class="mono">${esc(r.ip)}</td>
      <td>${esc(r.hostname || '-')}</td>
      <td class="mono">${esc(r.gateway || '-')}</td>
      <td class="mono">${r.dns_servers ? esc(r.dns_servers.join(', ')) : '-'}</td>
      <td>${r.next_server ? '<span class="badge ok">PXE</span>' : ''}</td>
      <td>
        <button class="btn btn-ghost btn-xs" onclick="editRes('${esc(r.mac)}')" title="Edit">&#9998;</button>
        <button class="btn btn-danger btn-xs" onclick="confirmDeleteRes('${esc(r.mac)}')" title="Delete">&times;</button>
      </td>
    </tr>`
  ).join('') || '<tr><td colspan="7" style="color:var(--text-muted)">No reservations</td></tr>';
}

function filterLeases() {
  const q = (document.getElementById('lease-search').value || '').toLowerCase();
  const filtered = q ? allLeases.filter(l =>
    (l.ip_addr||'').toLowerCase().includes(q) ||
    (l.mac_addr||'').toLowerCase().includes(q) ||
    (l.hostname||'').toLowerCase().includes(q)
  ) : allLeases;

  document.getElementById('dhcp-leases-table').innerHTML = filtered.map(l =>
    `<tr>
      <td class="mono">${esc(l.ip_addr)}</td>
      <td class="mono">${esc(l.mac_addr)}</td>
      <td>${esc(l.hostname||'-')}</td>
      <td style="font-size:11px">${new Date(l.lease_start).toLocaleString()}</td>
      <td title="${new Date(l.lease_end).toLocaleString()}">${relTime(l.lease_end)}</td>
      <td><span class="badge ${l.state==='Active'?'ok':'err'}">${esc(l.state)}</span></td>
    </tr>`
  ).join('') || '<tr><td colspan="6" style="color:var(--text-muted)">No active leases</td></tr>';
}

// ─── Pool Modal ───

function showPoolModal(pool) {
  editPoolId = pool ? pool.id : null;
  document.getElementById('pool-modal-title').textContent = pool ? 'Edit Pool' : 'Add Pool';
  document.getElementById('pool-modal-submit').textContent = pool ? 'Save' : 'Create';
  document.getElementById('pool-modal-error').style.display = 'none';

  document.getElementById('pm-name').value = pool ? pool.name : '';
  document.getElementById('pm-range-start').value = pool ? pool.range_start : '';
  document.getElementById('pm-range-end').value = pool ? pool.range_end : '';
  document.getElementById('pm-subnet').value = pool ? pool.subnet : '';
  document.getElementById('pm-gateway').value = pool ? pool.gateway : '';
  document.getElementById('pm-dns').value = pool ? arrToCsv(pool.dns_servers) : '';
  document.getElementById('pm-domain').value = pool ? pool.domain : '';
  document.getElementById('pm-lease').value = pool ? pool.lease_time_secs : 3600;
  document.getElementById('pm-mtu').value = pool && pool.mtu ? pool.mtu : '';
  document.getElementById('pm-ntp').value = pool ? arrToCsv(pool.ntp_servers) : '';
  document.getElementById('pm-next-server').value = pool ? (pool.next_server||'') : '';
  document.getElementById('pm-boot-file').value = pool ? (pool.boot_file||'') : '';
  document.getElementById('pm-boot-file-efi').value = pool ? (pool.boot_file_efi||'') : '';
  document.getElementById('pm-ipxe-url').value = pool ? (pool.ipxe_boot_url||'') : '';

  document.getElementById('pool-modal').style.display = 'flex';
}
function closePoolModal() { document.getElementById('pool-modal').style.display = 'none'; }

function editPool(id) {
  const pool = allPools.find(p => p.id === id);
  if (pool) showPoolModal(pool);
}

async function submitPool() {
  const errEl = document.getElementById('pool-modal-error');
  errEl.style.display = 'none';

  const body = {
    name: document.getElementById('pm-name').value.trim(),
    range_start: document.getElementById('pm-range-start').value.trim(),
    range_end: document.getElementById('pm-range-end').value.trim(),
    subnet: document.getElementById('pm-subnet').value.trim(),
    gateway: document.getElementById('pm-gateway').value.trim(),
    dns_servers: csvToArr(document.getElementById('pm-dns').value),
    domain: document.getElementById('pm-domain').value.trim(),
    lease_time_secs: parseInt(document.getElementById('pm-lease').value) || 3600,
  };

  const mtu = document.getElementById('pm-mtu').value.trim();
  if (mtu) body.mtu = parseInt(mtu);
  const ntp = csvToArr(document.getElementById('pm-ntp').value);
  if (ntp.length) body.ntp_servers = ntp;
  const ns = document.getElementById('pm-next-server').value.trim();
  if (ns) body.next_server = ns;
  const bf = document.getElementById('pm-boot-file').value.trim();
  if (bf) body.boot_file = bf;
  const bfe = document.getElementById('pm-boot-file-efi').value.trim();
  if (bfe) body.boot_file_efi = bfe;
  const ipxe = document.getElementById('pm-ipxe-url').value.trim();
  if (ipxe) body.ipxe_boot_url = ipxe;

  try {
    if (editPoolId) {
      await apiPatch(`/dhcp/pools/${editPoolId}`, body);
    } else {
      await apiPost('/dhcp/pools', body);
    }
    closePoolModal();
    await loadDhcp();
  } catch(e) {
    errEl.textContent = e.message;
    errEl.style.display = '';
  }
}

function confirmDeletePool(id, name) {
  showConfirm('Delete Pool', `Delete pool "${name}"?`, async () => {
    try { await apiDelete('/dhcp/pools/' + id); await loadDhcp(); } catch(e) { alert('Failed: ' + e.message); }
  });
}

// ─── Reservation Modal ───

function showResModal(res) {
  editResMac = res ? res.mac : null;
  document.getElementById('res-modal-title').textContent = res ? 'Edit Reservation' : 'Add Reservation';
  document.getElementById('res-modal-submit').textContent = res ? 'Save' : 'Create';
  document.getElementById('res-modal-error').style.display = 'none';

  const macInput = document.getElementById('rm-mac');
  macInput.value = res ? res.mac : '';
  macInput.disabled = !!res;

  document.getElementById('rm-ip').value = res ? res.ip : '';
  document.getElementById('rm-hostname').value = res ? (res.hostname||'') : '';
  document.getElementById('rm-domain').value = res ? (res.domain||'') : '';
  document.getElementById('rm-gateway').value = res ? (res.gateway||'') : '';
  document.getElementById('rm-dns').value = res ? arrToCsv(res.dns_servers) : '';
  document.getElementById('rm-lease').value = res && res.lease_time_secs ? res.lease_time_secs : '';
  document.getElementById('rm-mtu').value = res && res.mtu ? res.mtu : '';
  document.getElementById('rm-ntp').value = res ? arrToCsv(res.ntp_servers) : '';
  document.getElementById('rm-next-server').value = res ? (res.next_server||'') : '';
  document.getElementById('rm-boot-file').value = res ? (res.boot_file||'') : '';
  document.getElementById('rm-boot-file-efi').value = res ? (res.boot_file_efi||'') : '';
  document.getElementById('rm-ipxe-url').value = res ? (res.ipxe_boot_url||'') : '';

  document.getElementById('res-modal').style.display = 'flex';
}
function closeResModal() { document.getElementById('res-modal').style.display = 'none'; document.getElementById('rm-mac').disabled = false; }

function editRes(mac) {
  const res = allReservations.find(r => r.mac === mac);
  if (res) showResModal(res);
}

async function submitRes() {
  const errEl = document.getElementById('res-modal-error');
  errEl.style.display = 'none';

  const body = {
    ip: document.getElementById('rm-ip').value.trim(),
  };

  if (!editResMac) {
    body.mac = document.getElementById('rm-mac').value.trim();
    if (!body.mac) { errEl.textContent = 'MAC address required'; errEl.style.display = ''; return; }
  }
  if (!body.ip) { errEl.textContent = 'IP address required'; errEl.style.display = ''; return; }

  const hostname = document.getElementById('rm-hostname').value.trim();
  if (hostname) body.hostname = hostname; else if (editResMac) body.hostname = null;
  const domain = document.getElementById('rm-domain').value.trim();
  if (domain) body.domain = domain; else if (editResMac) body.domain = null;
  const gateway = document.getElementById('rm-gateway').value.trim();
  if (gateway) body.gateway = gateway; else if (editResMac) body.gateway = null;
  const dns = csvToArr(document.getElementById('rm-dns').value);
  if (dns.length) body.dns_servers = dns; else if (editResMac) body.dns_servers = null;
  const lease = document.getElementById('rm-lease').value.trim();
  if (lease) body.lease_time_secs = parseInt(lease); else if (editResMac) body.lease_time_secs = null;
  const mtu = document.getElementById('rm-mtu').value.trim();
  if (mtu) body.mtu = parseInt(mtu); else if (editResMac) body.mtu = null;
  const ntp = csvToArr(document.getElementById('rm-ntp').value);
  if (ntp.length) body.ntp_servers = ntp; else if (editResMac) body.ntp_servers = null;
  const ns = document.getElementById('rm-next-server').value.trim();
  if (ns) body.next_server = ns; else if (editResMac) body.next_server = null;
  const bf = document.getElementById('rm-boot-file').value.trim();
  if (bf) body.boot_file = bf; else if (editResMac) body.boot_file = null;
  const bfe = document.getElementById('rm-boot-file-efi').value.trim();
  if (bfe) body.boot_file_efi = bfe; else if (editResMac) body.boot_file_efi = null;
  const ipxe = document.getElementById('rm-ipxe-url').value.trim();
  if (ipxe) body.ipxe_boot_url = ipxe; else if (editResMac) body.ipxe_boot_url = null;

  try {
    if (editResMac) {
      await apiPatch(`/dhcp/reservations/${encodeURIComponent(editResMac)}`, body);
    } else {
      await apiPost('/dhcp/reservations', body);
    }
    closeResModal();
    await loadDhcp();
  } catch(e) {
    errEl.textContent = e.message;
    errEl.style.display = '';
  }
}

function confirmDeleteRes(mac) {
  showConfirm('Delete Reservation', `Delete reservation for ${mac}?`, async () => {
    try { await apiDelete('/dhcp/reservations/' + encodeURIComponent(mac)); await loadDhcp(); } catch(e) { alert('Failed: ' + e.message); }
  });
}

// ─── Events Tab ───

function renderEvents() {
  const filter = document.getElementById('event-filter').value;
  const filtered = filter ? events.filter(e => e.type === filter) : events;
  const feed = document.getElementById('event-feed');

  if (filtered.length === 0) {
    feed.innerHTML = '<div class="empty-state">Waiting for events...</div>';
    return;
  }

  feed.innerHTML = filtered.map(e => {
    const action = e.action || '';
    const actionClass = action.toLowerCase();
    let detail = '';
    switch(e.type) {
      case 'DhcpPoolChanged': detail = `Pool: ${esc(e.pool_name||e.pool_id)}`; break;
      case 'DhcpReservationChanged': detail = `${esc(e.mac)} → ${esc(e.ip)}`; break;
      case 'DnsForwarderChanged': detail = `Zone: ${esc(e.zone)}`; break;
      case 'LeaseChanged': detail = `${esc(e.ip)} (${esc(e.mac)})`; break;
      case 'ZoneChanged': detail = `Zone: ${esc(e.zone_name||e.zone_id)}`; break;
      case 'RecordChanged': detail = `Record: ${esc(e.record_name)} in ${esc(e.zone_id)}`; break;
      default: detail = JSON.stringify(e);
    }
    return `<div class="event-item">
      <span class="ts">${e._time ? new Date(e._time).toLocaleTimeString() : '-'}</span>
      <span><span class="event-type-badge">${esc(e.type.replace('Changed',''))}</span> <span class="action ${actionClass}">${esc(action)}</span></span>
      <span class="detail">${detail}</span>
    </div>`;
  }).join('');

  if (document.getElementById('event-auto-scroll').checked) {
    feed.scrollTop = 0;
  }
}

function clearEvents() {
  events = [];
  document.getElementById('event-count').style.display = 'none';
  renderEvents();
}

// ─── Logs ───

async function loadLogs() {
  const level = document.getElementById('log-level').value;
  const module = document.getElementById('log-module').value.trim();
  let path = '/logs?limit=200';
  if (level) path += '&level=' + level;
  if (module) path += '&module=' + encodeURIComponent(module);
  try {
    const data = await apiFetch(path);
    const entries = data.entries || [];
    document.getElementById('log-entries').innerHTML = entries.map(e => {
      const lvl = (e.level||'').toLowerCase();
      return `<div class="log-entry ${lvl}">
        <span class="ts">${esc(e.timestamp||'')}</span>
        <span class="lvl">${esc((e.level||'').toUpperCase())}</span>
        <span class="mod">${esc(e.module||'')}</span>
        <span class="msg">${esc(e.message||'')}</span>
      </div>`;
    }).join('') || '<div class="empty-state">No log entries</div>';
  } catch(e) {
    document.getElementById('log-entries').innerHTML = `<div class="empty-state">Error: ${esc(e.message)}</div>`;
  }
}

function toggleLogAuto() {
  if (intervals.logs) { clearInterval(intervals.logs); delete intervals.logs; }
  if (document.getElementById('log-auto').checked) intervals.logs = setInterval(loadLogs, 3000);
}

// ─── Peers ───

async function loadPeers() {
  try {
    const c = await apiFetch('/connectivity');
    if (!c.peers || c.peers.length === 0) {
      document.getElementById('peer-cards').innerHTML = '<div class="empty-state">No peers configured</div>';
      return;
    }
    document.getElementById('peer-cards').innerHTML = c.peers.map(p => {
      const probes = [{name:'DNS UDP',r:p.dns_udp},{name:'DNS TCP',r:p.dns_tcp},{name:'HTTP',r:p.http}];
      return `<div class="peer-card">
        <h3>${esc(p.id)}</h3>
        <div class="addr">${esc(p.addr)}</div>
        ${probes.map(pr => `<div class="probe-row">
          <span class="probe-label">${pr.name}</span>
          <span class="probe-result">
            <span class="dot ${pr.r.ok?'green':'red'}"></span>
            ${pr.r.ok ? `<span class="probe-latency">${pr.r.latency_ms.toFixed(1)} ms</span>` : `<span style="color:var(--red);font-size:11px">${esc(pr.r.error)}</span>`}
          </span>
        </div>`).join('')}
      </div>`;
    }).join('');
  } catch(e) {
    document.getElementById('peer-cards').innerHTML = `<div class="empty-state">Error: ${esc(e.message)}</div>`;
  }
}

// ─── Init ───

connect();
startOverview();
</script>
</body>
</html>"##;
