use axum::response::Html;

pub async fn dashboard_page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>MicroDNS Dashboard</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0f172a; color: #e2e8f0; }
  .header { background: #1e293b; padding: 16px 24px; border-bottom: 1px solid #334155; display: flex; justify-content: space-between; align-items: center; }
  .header h1 { font-size: 20px; font-weight: 600; }
  .status { display: flex; align-items: center; gap: 8px; font-size: 14px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; }
  .dot.green { background: #22c55e; }
  .dot.red { background: #ef4444; }
  .dot.yellow { background: #eab308; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; padding: 24px; }
  .card { background: #1e293b; border: 1px solid #334155; border-radius: 8px; padding: 16px; }
  .card h2 { font-size: 14px; text-transform: uppercase; letter-spacing: 0.05em; color: #94a3b8; margin-bottom: 12px; }
  .card.full { grid-column: 1 / -1; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; padding: 8px; color: #94a3b8; border-bottom: 1px solid #334155; font-weight: 500; }
  td { padding: 8px; border-bottom: 1px solid #1e293b; }
  .badge { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px; font-weight: 600; }
  .badge.healthy { background: #166534; color: #22c55e; }
  .badge.unhealthy { background: #7f1d1d; color: #ef4444; }
  .stat { text-align: center; }
  .stat .value { font-size: 32px; font-weight: 700; color: #f8fafc; }
  .stat .label { font-size: 12px; color: #94a3b8; margin-top: 4px; }
  .stats { display: flex; gap: 24px; justify-content: center; }
  #connection-status { font-size: 12px; }
</style>
</head>
<body>
<div class="header">
  <h1>MicroDNS Dashboard</h1>
  <div class="status">
    <span class="dot" id="ws-dot"></span>
    <span id="connection-status">Connecting...</span>
  </div>
</div>

<div class="grid">
  <div class="card">
    <h2>Overview</h2>
    <div class="stats">
      <div class="stat"><div class="value" id="zone-count">-</div><div class="label">Zones</div></div>
      <div class="stat"><div class="value" id="lease-count">-</div><div class="label">Active Leases</div></div>
      <div class="stat"><div class="value" id="instance-count">-</div><div class="label">Instances</div></div>
    </div>
  </div>

  <div class="card">
    <h2>Zones</h2>
    <table>
      <thead><tr><th>Name</th><th>Records</th></tr></thead>
      <tbody id="zones-table"></tbody>
    </table>
  </div>

  <div class="card full">
    <h2>Active Leases</h2>
    <table>
      <thead><tr><th>IP Address</th><th>MAC Address</th><th>Hostname</th><th>Expires</th></tr></thead>
      <tbody id="leases-table"></tbody>
    </table>
  </div>

  <div class="card full" id="instances-card" style="display:none">
    <h2>Cluster Instances</h2>
    <table>
      <thead><tr><th>Instance</th><th>Mode</th><th>Status</th><th>Leases</th></tr></thead>
      <tbody id="instances-table"></tbody>
    </table>
  </div>
</div>

<script>
let ws;
function connect() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  ws = new WebSocket(proto + '//' + location.host + '/ws');
  const dot = document.getElementById('ws-dot');
  const status = document.getElementById('connection-status');

  ws.onopen = () => { dot.className = 'dot green'; status.textContent = 'Connected'; };
  ws.onclose = () => { dot.className = 'dot red'; status.textContent = 'Disconnected'; setTimeout(connect, 3000); };
  ws.onerror = () => { dot.className = 'dot red'; status.textContent = 'Error'; };

  ws.onmessage = (evt) => {
    try {
      const data = JSON.parse(evt.data);
      updateDashboard(data);
    } catch(e) { console.error('parse error', e); }
  };
}

function updateDashboard(data) {
  document.getElementById('zone-count').textContent = data.zones.length;
  document.getElementById('lease-count').textContent = data.leases.length;
  document.getElementById('instance-count').textContent = data.instances.length;

  const zt = document.getElementById('zones-table');
  zt.innerHTML = data.zones.map(z =>
    `<tr><td>${esc(z.name)}</td><td>${z.record_count}</td></tr>`
  ).join('');

  const lt = document.getElementById('leases-table');
  lt.innerHTML = data.leases.map(l =>
    `<tr><td>${esc(l.ip_addr)}</td><td>${esc(l.mac_addr)}</td><td>${esc(l.hostname || '-')}</td><td>${new Date(l.lease_end).toLocaleString()}</td></tr>`
  ).join('');

  if (data.instances.length > 0) {
    document.getElementById('instances-card').style.display = '';
    const it = document.getElementById('instances-table');
    it.innerHTML = data.instances.map(i =>
      `<tr><td>${esc(i.instance_id)}</td><td>${esc(i.mode)}</td><td><span class="badge ${i.healthy ? 'healthy' : 'unhealthy'}">${i.healthy ? 'Healthy' : 'Down'}</span></td><td>${i.active_leases}</td></tr>`
    ).join('');
  }
}

function esc(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }

connect();
</script>
</body>
</html>"#;
