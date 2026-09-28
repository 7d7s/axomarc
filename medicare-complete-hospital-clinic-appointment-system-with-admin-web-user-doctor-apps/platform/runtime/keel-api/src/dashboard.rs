//! Horizon-lite dashboard (S10 B-20): a single static page served by keel-api
//! over the JSON meta surface (/healthz, /readyz, /meta/*, /metrics). No JS
//! framework, no build step, no extra dependency — mirrors the SPA starter
//! posture from S9. Read-only by design.

/// The complete dashboard page. Fetches boot/flags/routes/health and (when
/// enabled) /metrics, rendering each as a card. Failures degrade to "n/a"
/// so a disabled /metrics never breaks the page.
pub const DASHBOARD_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Keel Dashboard</title>
<style>
  :root { color-scheme: dark; --bg:#0b0e14; --card:#141a24; --edge:#232c3b;
          --fg:#dbe4f0; --dim:#7d8aa0; --ok:#3fb96f; --warn:#d9a441; --err:#d95757;
          --accent:#4f8cff; }
  * { box-sizing: border-box; }
  body { margin:0; background:var(--bg); color:var(--fg);
         font:14px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  header { display:flex; align-items:baseline; gap:12px; padding:20px 28px 8px; }
  header h1 { font-size:20px; margin:0; letter-spacing:.4px; }
  header .sub { color:var(--dim); font-size:12px; }
  .badge { display:inline-block; padding:2px 10px; border-radius:999px; font-size:12px;
           border:1px solid var(--edge); background:var(--card); }
  .badge.ok { color:var(--ok); border-color:var(--ok); }
  .badge.err { color:var(--err); border-color:var(--err); }
  main { display:grid; grid-template-columns:repeat(auto-fit,minmax(320px,1fr));
         gap:16px; padding:16px 28px 32px; }
  section { background:var(--card); border:1px solid var(--edge); border-radius:10px;
            padding:16px; min-height:120px; }
  section h2 { font-size:13px; text-transform:uppercase; letter-spacing:1px;
               color:var(--dim); margin:0 0 10px; }
  table { width:100%; border-collapse:collapse; font-size:13px; }
  td, th { text-align:left; padding:3px 6px; border-bottom:1px solid var(--edge);
           vertical-align:top; }
  th { color:var(--dim); font-weight:500; }
  code, pre { font-family:ui-monospace, Consolas, monospace; }
  pre { white-space:pre-wrap; word-break:break-all; margin:0; font-size:12px;
        color:var(--fg); max-height:280px; overflow:auto; }
  .pill { display:inline-block; margin:2px 4px 2px 0; padding:1px 8px; border-radius:6px;
          background:#1c2534; border:1px solid var(--edge); font-size:12px; }
  .k { color:var(--accent); }
  .empty { color:var(--dim); font-style:italic; }
</style>
</head>
<body>
<header>
  <h1>Keel</h1>
  <span class="sub">Horizon-lite runtime dashboard</span>
  <span id="health" class="badge">…</span>
  <span id="ready" class="badge">…</span>
</header>
<main>
  <section><h2>Boot</h2><div id="boot" class="empty">loading…</div></section>
  <section><h2>Feature flags</h2><div id="flags" class="empty">loading…</div></section>
  <section><h2>Graph routes</h2><div id="routes" class="empty">loading…</div></section>
  <section><h2>Services</h2><div id="services" class="empty">loading…</div></section>
  <section style="grid-column:1/-1"><h2>Metrics (Prometheus)</h2><div id="metrics" class="empty">loading…</div></section>
</main>
<script>
async function j(url){ const r = await fetch(url); return { status:r.status, json: await r.json().catch(()=>null) }; }
function set(id, html){ document.getElementById(id).innerHTML = html; }
function esc(s){ return String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }
async function load(){
  try {
    const h = await j("/healthz");
    const ok = h.json && h.json.data && h.json.data.ok;
    set("health", ""); const hb = document.getElementById("health");
    hb.textContent = ok ? "healthy" : (h.json && h.json.data ? "degraded" : "unknown");
    hb.className = "badge " + (ok ? "ok" : "err");
  } catch { set("health","n/a"); }
  try {
    const r = await fetch("/readyz");
    const rb = document.getElementById("ready");
    rb.textContent = r.status === 200 ? "ready" : "not ready";
    rb.className = "badge " + (r.status === 200 ? "ok" : "err");
  } catch { set("ready","n/a"); }
  try {
    const b = await j("/meta/boot");
    const d = b.json.data || {};
    set("boot", "<table><tr><th>name</th><td>" + esc(d.name||"") + "</td></tr>" +
                "<tr><th>version</th><td>" + esc(d.version||"") + "</td></tr></table>");
    const services = d.services || [];
    set("services", services.length
      ? "<table>" + services.map(s=>"<tr><td><code>"+esc(s)+"</code></td></tr>").join("") + "</table>"
      : '<span class="empty">none</span>');
  } catch { set("boot","n/a"); set("services","n/a"); }
  try {
    const f = await j("/meta/flags");
    const flags = (f.json.data || {}).enabled || [];
    set("flags", flags.length
      ? flags.map(x=>'<span class="pill">'+esc(x)+"</span>").join("")
      : '<span class="empty">none enabled</span>');
  } catch { set("flags","n/a"); }
  try {
    const rt = await j("/meta/routes");
    const routes = (rt.json.data || {}).routes || [];
    set("routes", routes.length
      ? "<table><tr><th>method</th><th>path</th><th>operation</th></tr>" +
        routes.map(r=>"<tr><td>"+esc(r.method)+"</td><td><code>"+esc(r.path)+"</code></td><td>"+esc(r.operation||"")+"</td></tr>").join("") +
        "</table>"
      : '<span class="empty">no graph routes</span>');
  } catch { set("routes","n/a"); }
  try {
    const m = await fetch("/metrics");
    if (m.status === 200) {
      const text = await m.text();
      set("metrics", "<pre>" + esc(text || "# (no series yet)") + "</pre>");
    } else {
      set("metrics", '<span class="empty">disabled (flag app.metrics off) — HTTP ' + m.status + "</span>");
    }
  } catch { set("metrics","n/a"); }
}
load(); setInterval(load, 5000);
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_html_is_well_formed_enough() {
        assert!(DASHBOARD_HTML.starts_with("<!doctype html>"));
        assert!(DASHBOARD_HTML.contains("/meta/boot"));
        assert!(DASHBOARD_HTML.contains("/metrics"));
        assert!(DASHBOARD_HTML.contains("/_dashboard") || DASHBOARD_HTML.contains("Keel"));
    }
}
