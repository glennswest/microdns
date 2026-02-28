use axum::response::Html;

pub async fn dashboard_page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>MicroDNS Dashboard</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#0f172a;color:#e2e8f0;min-height:100vh}
.header{background:#1e293b;padding:12px 24px;border-bottom:1px solid #334155;display:flex;justify-content:space-between;align-items:center}
.header h1{font-size:18px;font-weight:600}
.status{display:flex;align-items:center;gap:8px;font-size:13px;color:#94a3b8}
.dot{width:8px;height:8px;border-radius:50%;display:inline-block}
.dot.green{background:#22c55e}.dot.red{background:#ef4444}.dot.yellow{background:#eab308}.dot.gray{background:#64748b}

/* Tabs */
.tabs{display:flex;gap:0;background:#1e293b;border-bottom:1px solid #334155;padding:0 24px}
.tab{padding:10px 20px;font-size:13px;font-weight:500;color:#94a3b8;cursor:pointer;border-bottom:2px solid transparent;transition:all .15s}
.tab:hover{color:#e2e8f0}.tab.active{color:#3b82f6;border-bottom-color:#3b82f6}

/* Layout */
.content{padding:20px 24px;display:none}.content.active{display:block}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:16px;margin-bottom:20px}
.card{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:16px}
.card h2{font-size:12px;text-transform:uppercase;letter-spacing:.05em;color:#64748b;margin-bottom:10px}
.card.full{grid-column:1/-1}

/* Stats */
.stat{text-align:center}
.stat .value{font-size:28px;font-weight:700;color:#f8fafc}
.stat .label{font-size:11px;color:#94a3b8;margin-top:2px}

/* Tables */
table{width:100%;border-collapse:collapse;font-size:13px}
th{text-align:left;padding:6px 8px;color:#64748b;border-bottom:1px solid #334155;font-weight:500;font-size:11px;text-transform:uppercase;letter-spacing:.04em}
td{padding:6px 8px;border-bottom:1px solid #1e293b}
tr.clickable{cursor:pointer}tr.clickable:hover{background:#334155}
tr.selected{background:#1e3a5f}

/* Badges */
.badge{display:inline-block;padding:1px 6px;border-radius:3px;font-size:11px;font-weight:600}
.badge.ok{background:#166534;color:#22c55e}.badge.err{background:#7f1d1d;color:#ef4444}
.badge.active{background:#1e3a5f;color:#3b82f6}
.enabled-dot{width:8px;height:8px;border-radius:50%;display:inline-block}
.enabled-dot.on{background:#22c55e}.enabled-dot.off{background:#ef4444}

/* Forms */
input,select{background:#0f172a;border:1px solid #334155;color:#e2e8f0;padding:6px 10px;border-radius:4px;font-size:13px;font-family:inherit}
input:focus,select:focus{outline:none;border-color:#3b82f6}
input::placeholder{color:#475569}
.form-row{display:flex;gap:8px;align-items:center;margin-bottom:12px;flex-wrap:wrap}

/* Buttons */
.btn{padding:6px 14px;border:none;border-radius:4px;font-size:12px;font-weight:500;cursor:pointer;transition:background .15s}
.btn-primary{background:#3b82f6;color:#fff}.btn-primary:hover{background:#2563eb}
.btn-danger{background:#7f1d1d;color:#ef4444}.btn-danger:hover{background:#991b1b}
.btn-sm{padding:3px 8px;font-size:11px}
.btn-ghost{background:transparent;color:#94a3b8;border:1px solid #334155}.btn-ghost:hover{background:#334155}

/* DNS layout */
.dns-layout{display:grid;grid-template-columns:300px 1fr;gap:20px;min-height:400px}
.zone-list{background:#1e293b;border:1px solid #334155;border-radius:8px;overflow:hidden}
.zone-list-header{padding:10px 12px;border-bottom:1px solid #334155;display:flex;justify-content:space-between;align-items:center}
.zone-list-header h2{font-size:12px;text-transform:uppercase;letter-spacing:.05em;color:#64748b;margin:0}
.zone-item{padding:8px 12px;cursor:pointer;border-bottom:1px solid #1e293b;font-size:13px;display:flex;justify-content:space-between;align-items:center}
.zone-item:hover{background:#334155}.zone-item.selected{background:#1e3a5f}
.zone-item .name{font-weight:500}.zone-item .count{color:#64748b;font-size:11px}
.records-panel{background:#1e293b;border:1px solid #334155;border-radius:8px;overflow:hidden}
.records-header{padding:10px 12px;border-bottom:1px solid #334155;display:flex;justify-content:space-between;align-items:center}
.records-header h2{font-size:12px;text-transform:uppercase;letter-spacing:.05em;color:#64748b;margin:0}
.records-body{padding:12px;overflow-x:auto}
.empty-state{color:#64748b;text-align:center;padding:40px;font-size:13px}

/* Inline edit */
.edit-row td{background:#0f172a}
.edit-row input,.edit-row select{width:100%;padding:4px 6px;font-size:12px}

/* Logs */
.log-entry{font-family:'SF Mono',Monaco,Consolas,monospace;font-size:12px;padding:3px 0;border-bottom:1px solid #1e293b;display:grid;grid-template-columns:160px 50px 120px 1fr;gap:8px}
.log-entry .ts{color:#64748b}.log-entry .mod{color:#94a3b8}
.log-entry.error .lvl{color:#ef4444}.log-entry.warn .lvl{color:#eab308}
.log-entry.info .lvl{color:#94a3b8}.log-entry.debug .lvl{color:#475569}
.log-entry .msg{word-break:break-all}

/* Peers */
.peer-cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(320px,1fr));gap:16px}
.peer-card{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:16px}
.peer-card h3{font-size:14px;font-weight:500;margin-bottom:4px}
.peer-card .addr{color:#64748b;font-size:12px;margin-bottom:12px}
.probe-row{display:flex;justify-content:space-between;align-items:center;padding:4px 0;font-size:13px}
.probe-label{color:#94a3b8}
.probe-result{display:flex;align-items:center;gap:6px}
.probe-latency{color:#64748b;font-size:11px}

/* Modal/confirm overlay */
.confirm-overlay{position:fixed;inset:0;background:rgba(0,0,0,.5);display:flex;align-items:center;justify-content:center;z-index:100}
.confirm-box{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:24px;max-width:400px;width:90%}
.confirm-box h3{margin-bottom:12px;font-size:15px}
.confirm-box p{color:#94a3b8;font-size:13px;margin-bottom:16px}
.confirm-actions{display:flex;gap:8px;justify-content:flex-end}

/* Responsive */
@media(max-width:768px){
.dns-layout{grid-template-columns:1fr}
.log-entry{grid-template-columns:1fr;gap:2px}
}
</style>
</head>
<body>
<div class="header">
  <h1>MicroDNS</h1>
  <div class="status">
    <span class="dot" id="ws-dot"></span>
    <span id="connection-status">Connecting...</span>
  </div>
</div>

<div class="tabs">
  <div class="tab active" onclick="switchTab('overview')">Overview</div>
  <div class="tab" onclick="switchTab('dns')">DNS</div>
  <div class="tab" onclick="switchTab('lb')">Load Balancer</div>
  <div class="tab" onclick="switchTab('dhcp')">DHCP</div>
  <div class="tab" onclick="switchTab('logs')">Logs</div>
  <div class="tab" onclick="switchTab('peers')">Peers</div>
</div>

<!-- OVERVIEW TAB -->
<div class="content active" id="tab-overview">
  <div class="grid">
    <div class="card stat"><div class="value" id="zone-count">-</div><div class="label">Zones</div></div>
    <div class="card stat"><div class="value" id="lease-count">-</div><div class="label">Active Leases</div></div>
    <div class="card stat"><div class="value" id="instance-count">-</div><div class="label">Instances</div></div>
  </div>
  <div class="grid" style="grid-template-columns:1fr 1fr">
    <div class="card">
      <h2>Health</h2>
      <table>
        <tr><td>Status</td><td id="health-status">-</td></tr>
        <tr><td>Version</td><td id="health-version">-</td></tr>
      </table>
    </div>
    <div class="card">
      <h2>Peer Connectivity</h2>
      <div id="connectivity-summary"><span style="color:#64748b">Loading...</span></div>
    </div>
  </div>
  <div class="card" id="instances-card" style="display:none">
    <h2>Cluster Instances</h2>
    <table>
      <thead><tr><th>Instance</th><th>Mode</th><th>Status</th><th>Leases</th></tr></thead>
      <tbody id="instances-table"></tbody>
    </table>
  </div>
  <div class="card">
    <h2>Zones</h2>
    <table>
      <thead><tr><th>Name</th><th>Records</th></tr></thead>
      <tbody id="overview-zones-table"></tbody>
    </table>
  </div>
</div>

<!-- DNS TAB -->
<div class="content" id="tab-dns">
  <div class="dns-layout">
    <div class="zone-list">
      <div class="zone-list-header">
        <h2>Zones</h2>
        <button class="btn btn-primary btn-sm" onclick="showAddZone()">+ Add Zone</button>
      </div>
      <div id="add-zone-form" style="display:none;padding:8px 12px;border-bottom:1px solid #334155">
        <div class="form-row">
          <input type="text" id="new-zone-name" placeholder="zone name (e.g. example.com)" style="flex:1">
          <input type="number" id="new-zone-ttl" placeholder="TTL" value="300" style="width:70px">
          <button class="btn btn-primary btn-sm" onclick="createZone()">Create</button>
          <button class="btn btn-ghost btn-sm" onclick="hideAddZone()">Cancel</button>
        </div>
        <div id="zone-error" style="color:#ef4444;font-size:12px;display:none"></div>
      </div>
      <div id="zones-list-body"></div>
    </div>
    <div class="records-panel">
      <div class="records-header">
        <h2>Records <span id="records-zone-name" style="color:#e2e8f0;text-transform:none;letter-spacing:0;font-size:13px;font-weight:400"></span></h2>
        <button class="btn btn-primary btn-sm" id="add-record-btn" style="display:none" onclick="showAddRecord()">+ Add Record</button>
      </div>
      <div class="records-body" id="records-body">
        <div class="empty-state">Select a zone to view records</div>
      </div>
    </div>
  </div>
</div>

<!-- LOAD BALANCER TAB -->
<div class="content" id="tab-lb">
  <div style="margin-bottom:12px;display:flex;justify-content:space-between;align-items:center">
    <h2 style="font-size:14px;color:#94a3b8">Health-Checked Records</h2>
    <button class="btn btn-ghost btn-sm" onclick="loadLB()">Refresh</button>
  </div>
  <div class="grid" id="lb-summary" style="grid-template-columns:repeat(4,1fr);margin-bottom:16px">
    <div class="card stat"><div class="value" id="lb-total">-</div><div class="label">Health-Checked</div></div>
    <div class="card stat"><div class="value" id="lb-healthy" style="color:#22c55e">-</div><div class="label">Healthy</div></div>
    <div class="card stat"><div class="value" id="lb-unhealthy" style="color:#ef4444">-</div><div class="label">Unhealthy</div></div>
    <div class="card stat"><div class="value" id="lb-groups">-</div><div class="label">Failover Groups</div></div>
  </div>
  <div class="card" id="lb-groups-card" style="display:none;margin-bottom:16px">
    <h2>Failover Groups</h2>
    <div id="lb-groups-body"></div>
  </div>
  <div class="card">
    <h2>All Health-Checked Records</h2>
    <table>
      <thead><tr><th>Zone</th><th>Name</th><th>Type</th><th>Target</th><th>Probe</th><th>Interval</th><th>Thresholds</th><th>Status</th></tr></thead>
      <tbody id="lb-records-table"></tbody>
    </table>
  </div>
</div>

<!-- DHCP TAB -->
<div class="content" id="tab-dhcp">
  <div class="grid" style="grid-template-columns:repeat(4,1fr)">
    <div class="card stat"><div class="value" id="dhcp-enabled">-</div><div class="label">DHCP</div></div>
    <div class="card stat"><div class="value" id="dhcp-interface">-</div><div class="label">Interface</div></div>
    <div class="card stat"><div class="value" id="dhcp-reservations">-</div><div class="label">Reservations</div></div>
    <div class="card stat"><div class="value" id="dhcp-active">-</div><div class="label">Active Leases</div></div>
  </div>
  <div class="card" style="margin-bottom:20px">
    <h2>Pools</h2>
    <table>
      <thead><tr><th>Range</th><th>Subnet</th><th>Gateway</th><th>Domain</th><th>Lease Time</th><th>PXE</th></tr></thead>
      <tbody id="dhcp-pools-table"></tbody>
    </table>
  </div>
  <div class="card">
    <h2>Active Leases</h2>
    <div class="form-row" style="margin-bottom:8px">
      <input type="text" id="lease-search" placeholder="Search IP, MAC, or hostname..." style="width:300px" oninput="filterLeases()">
    </div>
    <table>
      <thead><tr><th>IP Address</th><th>MAC Address</th><th>Hostname</th><th>Lease Start</th><th>Lease End</th><th>State</th></tr></thead>
      <tbody id="dhcp-leases-table"></tbody>
    </table>
  </div>
</div>

<!-- LOGS TAB -->
<div class="content" id="tab-logs">
  <div class="form-row" style="margin-bottom:12px">
    <select id="log-level" onchange="loadLogs()">
      <option value="">All Levels</option>
      <option value="error">ERROR</option>
      <option value="warn">WARN</option>
      <option value="info" selected>INFO</option>
      <option value="debug">DEBUG</option>
    </select>
    <input type="text" id="log-module" placeholder="Module filter..." style="width:200px" onchange="loadLogs()">
    <label style="font-size:13px;display:flex;align-items:center;gap:6px;cursor:pointer">
      <input type="checkbox" id="log-auto" checked onchange="toggleLogAuto()"> Auto-refresh
    </label>
    <button class="btn btn-ghost btn-sm" onclick="loadLogs()">Refresh</button>
  </div>
  <div class="card" style="max-height:calc(100vh - 200px);overflow-y:auto">
    <div id="log-entries"></div>
  </div>
</div>

<!-- PEERS TAB -->
<div class="content" id="tab-peers">
  <div style="margin-bottom:12px;display:flex;justify-content:space-between;align-items:center">
    <h2 style="font-size:14px;color:#94a3b8">Peer Connectivity Probes</h2>
    <button class="btn btn-ghost btn-sm" onclick="loadPeers()">Refresh</button>
  </div>
  <div class="peer-cards" id="peer-cards"></div>
</div>

<!-- CONFIRM DIALOG -->
<div class="confirm-overlay" id="confirm-dialog" style="display:none">
  <div class="confirm-box">
    <h3 id="confirm-title">Confirm</h3>
    <p id="confirm-msg"></p>
    <div class="confirm-actions">
      <button class="btn btn-ghost" onclick="closeConfirm()">Cancel</button>
      <button class="btn btn-danger" id="confirm-ok" onclick="doConfirm()">Delete</button>
    </div>
  </div>
</div>

<script>
const API = `http://${location.hostname}:8080/api/v1`;
let ws, wsData = {zones:[],leases:[],instances:[]};
let selectedZoneId = null, selectedZoneName = '';
let allLeases = [];
let intervals = {};
let confirmCb = null;

// ─── Helpers ───

function esc(s) { if(!s) return ''; const d=document.createElement('div'); d.textContent=s; return d.innerHTML; }

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
  const abs = Math.abs(diff), s = Math.floor(abs/1000), m = Math.floor(s/60), h = Math.floor(m/60);
  if (diff < 0) return 'expired';
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
  document.querySelectorAll('.tab').forEach((t,i) => {
    const tabs = ['overview','dns','lb','dhcp','logs','peers'];
    t.classList.toggle('active', tabs[i] === name);
  });
  document.querySelectorAll('.content').forEach(c => c.classList.remove('active'));
  document.getElementById('tab-' + name).classList.add('active');

  // Stop all tab intervals
  Object.values(intervals).forEach(i => clearInterval(i));
  intervals = {};

  // Start tab-specific polling
  switch(name) {
    case 'overview': startOverview(); break;
    case 'dns': break; // uses WS data for zone list
    case 'lb': loadLB(); intervals.lb = setInterval(loadLB, 10000); break;
    case 'dhcp': loadDhcp(); intervals.dhcp = setInterval(loadDhcp, 5000); break;
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

  ws.onopen = () => { dot.className='dot green'; status.textContent='Connected'; };
  ws.onclose = () => { dot.className='dot red'; status.textContent='Disconnected'; setTimeout(connect,3000); };
  ws.onerror = () => { dot.className='dot red'; status.textContent='Error'; };

  ws.onmessage = (evt) => {
    try { wsData = JSON.parse(evt.data); updateFromWS(); } catch(e) {}
  };
}

function updateFromWS() {
  // Overview stats
  document.getElementById('zone-count').textContent = wsData.zones.length;
  document.getElementById('lease-count').textContent = wsData.leases.length;
  document.getElementById('instance-count').textContent = wsData.instances.length;

  // Overview zones table
  document.getElementById('overview-zones-table').innerHTML = wsData.zones
    .map(z => `<tr><td>${esc(z.name)}</td><td>${z.record_count}</td></tr>`).join('');

  // Instances
  if (wsData.instances.length > 0) {
    document.getElementById('instances-card').style.display = '';
    document.getElementById('instances-table').innerHTML = wsData.instances
      .map(i => `<tr><td>${esc(i.instance_id)}</td><td>${esc(i.mode)}</td><td><span class="badge ${i.healthy?'ok':'err'}">${i.healthy?'Healthy':'Down'}</span></td><td>${i.active_leases}</td></tr>`).join('');
  } else {
    document.getElementById('instances-card').style.display = 'none';
  }

  // DNS tab zone list
  renderZoneList();
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
      document.getElementById('connectivity-summary').innerHTML = '<span style="color:#64748b">No peers configured</span>';
      return;
    }
    document.getElementById('connectivity-summary').innerHTML = c.peers.map(p => {
      const probes = [
        {name:'UDP', r:p.dns_udp}, {name:'TCP', r:p.dns_tcp}, {name:'HTTP', r:p.http}
      ];
      const dots = probes.map(pr =>
        `<span class="dot ${pr.r.ok?'green':'red'}" title="${pr.name}: ${pr.r.ok ? pr.r.latency_ms.toFixed(1)+'ms' : pr.r.error}"></span>`
      ).join('');
      return `<div style="display:flex;align-items:center;gap:8px;padding:4px 0;font-size:13px">
        <span style="min-width:80px">${esc(p.id)}</span>${dots}
        <span style="color:#64748b;font-size:11px">${p.dns_udp.ok ? p.dns_udp.latency_ms.toFixed(1)+'ms' : ''}</span>
      </div>`;
    }).join('');
  } catch(e) {
    document.getElementById('connectivity-summary').innerHTML = '<span style="color:#64748b">Unavailable</span>';
  }
}

// ─── DNS Tab ───

function renderZoneList() {
  const body = document.getElementById('zones-list-body');
  body.innerHTML = wsData.zones.map(z =>
    `<div class="zone-item ${z.id===selectedZoneId?'selected':''}" onclick="selectZone('${z.id}','${esc(z.name)}')">
      <span class="name">${esc(z.name)}</span>
      <span style="display:flex;align-items:center;gap:8px">
        <span class="count">${z.record_count} records</span>
        <button class="btn btn-danger btn-sm" onclick="event.stopPropagation();confirmDeleteZone('${z.id}','${esc(z.name)}')" title="Delete zone">&times;</button>
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
    try {
      await apiDelete('/zones/' + id);
      if (selectedZoneId === id) { selectedZoneId = null; renderRecordsEmpty(); }
    } catch(e) { alert('Failed: ' + e.message); }
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
    document.getElementById('records-body').innerHTML = '<div class="empty-state">No records in this zone</div>';
    return;
  }
  // Sort: SOA first, then NS, then by name+type
  recs.sort((a,b) => {
    const order = {SOA:0,NS:1,A:2,AAAA:3,CNAME:4,MX:5,TXT:6,SRV:7,PTR:8,CAA:9};
    const oa = order[a.type] ?? 10, ob = order[b.type] ?? 10;
    if (oa !== ob) return oa - ob;
    return (a.name||'').localeCompare(b.name||'');
  });

  let html = `<table><thead><tr><th>Name</th><th>Type</th><th>Data</th><th>TTL</th><th>On</th><th style="width:80px"></th></tr></thead><tbody>`;
  html += `<tr id="add-record-row" style="display:none">${addRecordCells()}</tr>`;
  for (const r of recs) {
    html += `<tr id="rec-${r.id}">
      <td>${esc(r.name)}</td>
      <td><span class="badge active">${esc(r.type)}</span></td>
      <td style="font-family:monospace;font-size:12px">${esc(fmtRecordData(r.data))}</td>
      <td>${r.ttl}</td>
      <td><span class="enabled-dot ${r.enabled?'on':'off'}"></span></td>
      <td>
        <button class="btn btn-ghost btn-sm" onclick="startEditRecord('${r.id}')" title="Edit">&#9998;</button>
        <button class="btn btn-danger btn-sm" onclick="confirmDeleteRecord('${r.id}','${esc(r.name)}','${esc(r.type)}')" title="Delete">&times;</button>
      </td>
    </tr>`;
  }
  html += '</tbody></table>';
  document.getElementById('records-body').innerHTML = html;

  // Store records for inline edit
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
    <td><input type="number" id="nr-ttl" value="300" style="width:60px"></td>
    <td></td>
    <td>
      <button class="btn btn-primary btn-sm" onclick="createRecord()">Add</button>
      <button class="btn btn-ghost btn-sm" onclick="hideAddRecord()">X</button>
    </td>`;
}

function showAddRecord() {
  document.getElementById('add-record-row').style.display = '';
  updateRecordDataFields();
  const nameInput = document.getElementById('nr-name');
  if (nameInput) nameInput.focus();
}

function hideAddRecord() {
  document.getElementById('add-record-row').style.display = 'none';
}

function updateRecordDataFields() {
  const type = document.getElementById('nr-type').value;
  const cell = document.getElementById('nr-data-cell');
  switch(type) {
    case 'MX':
      cell.innerHTML = `<div style="display:flex;gap:4px"><input type="number" id="nr-mx-pref" placeholder="pref" value="10" style="width:50px"><input type="text" id="nr-mx-exch" placeholder="exchange" style="flex:1"></div>`;
      break;
    case 'SRV':
      cell.innerHTML = `<div style="display:flex;gap:4px"><input type="number" id="nr-srv-pri" placeholder="pri" value="10" style="width:45px"><input type="number" id="nr-srv-w" placeholder="wt" value="0" style="width:40px"><input type="number" id="nr-srv-port" placeholder="port" style="width:50px"><input type="text" id="nr-srv-target" placeholder="target" style="flex:1"></div>`;
      break;
    case 'CAA':
      cell.innerHTML = `<div style="display:flex;gap:4px"><input type="number" id="nr-caa-flags" value="0" style="width:40px"><select id="nr-caa-tag" style="width:80px"><option>issue</option><option>issuewild</option><option>iodef</option></select><input type="text" id="nr-caa-val" placeholder="value" style="flex:1"></div>`;
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
    try {
      await apiDelete(`/zones/${selectedZoneId}/records/${id}`);
      await loadRecords();
    } catch(e) { alert('Failed: ' + e.message); }
  });
}

function startEditRecord(id) {
  const r = window._records[id];
  if (!r) return;
  const row = document.getElementById('rec-' + id);
  if (!row) return;

  // Build data editing cell based on type
  let dataCell;
  switch(r.type) {
    case 'MX':
      dataCell = `<div style="display:flex;gap:4px"><input type="number" id="er-mx-pref" value="${r.data.data.preference}" style="width:50px"><input type="text" id="er-mx-exch" value="${esc(r.data.data.exchange)}" style="flex:1"></div>`;
      break;
    case 'SRV':
      dataCell = `<div style="display:flex;gap:4px"><input type="number" id="er-srv-pri" value="${r.data.data.priority}" style="width:45px"><input type="number" id="er-srv-w" value="${r.data.data.weight}" style="width:40px"><input type="number" id="er-srv-port" value="${r.data.data.port}" style="width:50px"><input type="text" id="er-srv-target" value="${esc(r.data.data.target)}" style="flex:1"></div>`;
      break;
    case 'CAA':
      dataCell = `<div style="display:flex;gap:4px"><input type="number" id="er-caa-flags" value="${r.data.data.flags}" style="width:40px"><select id="er-caa-tag" style="width:80px"><option ${r.data.data.tag==='issue'?'selected':''}>issue</option><option ${r.data.data.tag==='issuewild'?'selected':''}>issuewild</option><option ${r.data.data.tag==='iodef'?'selected':''}>iodef</option></select><input type="text" id="er-caa-val" value="${esc(r.data.data.value)}" style="flex:1"></div>`;
      break;
    default:
      dataCell = `<input type="text" id="er-data" value="${esc(typeof r.data.data==='string'?r.data.data:JSON.stringify(r.data.data))}" style="width:100%">`;
  }

  row.className = 'edit-row';
  row.innerHTML = `
    <td><input type="text" id="er-name" value="${esc(r.name)}" style="width:100%"></td>
    <td><span class="badge active">${esc(r.type)}</span></td>
    <td>${dataCell}</td>
    <td><input type="number" id="er-ttl" value="${r.ttl}" style="width:60px"></td>
    <td><select id="er-enabled"><option value="true" ${r.enabled?'selected':''}>On</option><option value="false" ${!r.enabled?'selected':''}>Off</option></select></td>
    <td>
      <button class="btn btn-primary btn-sm" onclick="saveEditRecord('${r.id}','${esc(r.type)}')">Save</button>
      <button class="btn btn-ghost btn-sm" onclick="loadRecords()">X</button>
    </td>`;
}

async function saveEditRecord(id, type) {
  const name = document.getElementById('er-name').value.trim();
  const ttl = parseInt(document.getElementById('er-ttl').value) || 300;
  const enabled = document.getElementById('er-enabled').value === 'true';
  const data = buildRecordData(type, 'er');
  try {
    await apiPut(`/zones/${selectedZoneId}/records/${id}`, {name, ttl, data, enabled});
    await loadRecords();
  } catch(e) { alert('Failed: ' + e.message); }
}

// ─── Load Balancer Tab ───

async function loadLB() {
  try {
    // Fetch records from all zones known via WS
    const zones = wsData.zones || [];
    const allRecs = [];
    await Promise.all(zones.map(async z => {
      try {
        const recs = await apiFetch(`/zones/${z.id}/records?limit=500`);
        recs.forEach(r => { r._zoneName = z.name; r._zoneId = z.id; });
        allRecs.push(...recs);
      } catch(e) {}
    }));

    // Filter to only records with health_check configured
    const hcRecs = allRecs.filter(r => r.health_check);
    const healthy = hcRecs.filter(r => r.enabled).length;
    const unhealthy = hcRecs.length - healthy;

    // Build failover groups: records sharing same (zone_id, name, type)
    const groupMap = {};
    hcRecs.forEach(r => {
      const key = `${r._zoneId}|${r.name}|${r.type}`;
      if (!groupMap[key]) groupMap[key] = {zone:r._zoneName, name:r.name, type:r.type, records:[]};
      groupMap[key].records.push(r);
    });
    const groups = Object.values(groupMap).filter(g => g.records.length >= 2);

    // Stats
    document.getElementById('lb-total').textContent = hcRecs.length;
    document.getElementById('lb-healthy').textContent = healthy;
    document.getElementById('lb-unhealthy').textContent = unhealthy;
    document.getElementById('lb-groups').textContent = groups.length;

    // Failover groups
    if (groups.length > 0) {
      document.getElementById('lb-groups-card').style.display = '';
      document.getElementById('lb-groups-body').innerHTML = groups.map(g => {
        const allDown = g.records.every(r => !r.enabled);
        return `<div style="margin-bottom:12px;padding:10px;background:#0f172a;border-radius:6px">
          <div style="display:flex;align-items:center;gap:8px;margin-bottom:6px">
            <span style="font-weight:500">${esc(g.name)}.${esc(g.zone)}</span>
            <span class="badge active">${esc(g.type)}</span>
            ${allDown ? '<span class="badge err">FAILSAFE</span>' : ''}
          </div>
          <div style="display:flex;gap:12px;flex-wrap:wrap">${g.records.map(r =>
            `<div style="display:flex;align-items:center;gap:6px;font-size:12px">
              <span class="dot ${r.enabled?'green':'red'}"></span>
              <span style="font-family:monospace">${esc(fmtRecordData(r.data))}</span>
            </div>`
          ).join('')}</div>
        </div>`;
      }).join('');
    } else {
      document.getElementById('lb-groups-card').style.display = 'none';
    }

    // All records table
    document.getElementById('lb-records-table').innerHTML = hcRecs.map(r => {
      const hc = r.health_check;
      return `<tr>
        <td>${esc(r._zoneName)}</td>
        <td>${esc(r.name)}</td>
        <td><span class="badge active">${esc(r.type)}</span></td>
        <td style="font-family:monospace;font-size:12px">${esc(fmtRecordData(r.data))}</td>
        <td>${esc(hc.probe_type)}</td>
        <td>${hc.interval_secs}s</td>
        <td style="font-size:11px;color:#94a3b8">${hc.healthy_threshold}/${hc.unhealthy_threshold}</td>
        <td><span class="badge ${r.enabled?'ok':'err'}">${r.enabled?'Healthy':'Down'}</span></td>
      </tr>`;
    }).join('') || '<tr><td colspan="8" style="color:#64748b">No health-checked records configured</td></tr>';
  } catch(e) {
    document.getElementById('lb-records-table').innerHTML = `<tr><td colspan="8" style="color:#64748b">Error: ${esc(e.message)}</td></tr>`;
  }
}

// ─── DHCP Tab ───

async function loadDhcp() {
  try {
    const [status, leases] = await Promise.all([apiFetch('/dhcp/status'), apiFetch('/leases?limit=500')]);
    document.getElementById('dhcp-enabled').textContent = status.enabled ? 'Enabled' : 'Disabled';
    document.getElementById('dhcp-interface').textContent = status.interface || '-';
    document.getElementById('dhcp-reservations').textContent = status.reservation_count;
    document.getElementById('dhcp-active').textContent = status.active_lease_count;

    document.getElementById('dhcp-pools-table').innerHTML = (status.pools || []).map(p =>
      `<tr><td>${esc(p.range_start)} — ${esc(p.range_end)}</td><td>${esc(p.subnet)}</td><td>${esc(p.gateway)}</td><td>${esc(p.domain)}</td><td>${p.lease_time_secs}s</td><td>${p.pxe_enabled ? '<span class="badge ok">PXE</span>' : ''}</td></tr>`
    ).join('') || '<tr><td colspan="6" style="color:#64748b">No pools configured</td></tr>';

    allLeases = leases;
    filterLeases();
  } catch(e) {}
}

function filterLeases() {
  const q = (document.getElementById('lease-search').value || '').toLowerCase();
  const filtered = q ? allLeases.filter(l =>
    (l.ip_addr||'').toLowerCase().includes(q) ||
    (l.mac_addr||'').toLowerCase().includes(q) ||
    (l.hostname||'').toLowerCase().includes(q)
  ) : allLeases;

  document.getElementById('dhcp-leases-table').innerHTML = filtered.map(l =>
    `<tr><td>${esc(l.ip_addr)}</td><td style="font-family:monospace;font-size:12px">${esc(l.mac_addr)}</td><td>${esc(l.hostname||'-')}</td><td>${new Date(l.lease_start).toLocaleString()}</td><td title="${new Date(l.lease_end).toLocaleString()}">${relTime(l.lease_end)}</td><td><span class="badge ${l.state==='Active'?'ok':'err'}">${esc(l.state)}</span></td></tr>`
  ).join('') || '<tr><td colspan="6" style="color:#64748b">No active leases</td></tr>';
}

// ─── Logs Tab ───

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
  if (document.getElementById('log-auto').checked) {
    intervals.logs = setInterval(loadLogs, 3000);
  }
}

// ─── Peers Tab ───

async function loadPeers() {
  try {
    const c = await apiFetch('/connectivity');
    if (!c.peers || c.peers.length === 0) {
      document.getElementById('peer-cards').innerHTML = '<div class="empty-state">No peers configured</div>';
      return;
    }
    document.getElementById('peer-cards').innerHTML = c.peers.map(p => {
      const probes = [
        {name:'DNS UDP', r:p.dns_udp}, {name:'DNS TCP', r:p.dns_tcp}, {name:'HTTP', r:p.http}
      ];
      return `<div class="peer-card">
        <h3>${esc(p.id)}</h3>
        <div class="addr">${esc(p.addr)}</div>
        ${probes.map(pr => `<div class="probe-row">
          <span class="probe-label">${pr.name}</span>
          <span class="probe-result">
            <span class="dot ${pr.r.ok?'green':'red'}"></span>
            ${pr.r.ok ? `<span class="probe-latency">${pr.r.latency_ms.toFixed(1)} ms</span>` : `<span style="color:#ef4444;font-size:12px">${esc(pr.r.error)}</span>`}
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
