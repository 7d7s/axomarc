// SPA-first starter (__PROJECT_NAME__).
// Auth model: same-origin cookie session for the browser (kelson_session),
// hashed PAT for non-browser API clients. The session cookie is HttpOnly;
// no token handling in JS for the web flow.
//
// Generate a typed OpenAPI client for your framework surface with:
//   kelson sdk:openapi

const API = {
  register: (email, password) =>
    post("/auth/register", { email, password, tenant: "dev", mode: "session" }),
  login: (email, password) =>
    post("/auth/login", { identifier: email, password, tenant: "dev", mode: "session" }),
  logout: () => post("/auth/logout", {}),
  me: () => get("/auth/me"),
  createNote: (body) => post("/api/v1/__PROJECT_NAME__/notes", { body }),
  listNotes: () => get("/api/v1/__PROJECT_NAME__/notes"),
};

async function post(path, data) {
  const res = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(data),
  });
  return res.json().catch(() => ({ ok: false, status: res.status }));
}

async function get(path) {
  const res = await fetch(path, { credentials: "same-origin", headers: { Accept: "application/json" } });
  return res.json().catch(() => ({ ok: false, status: res.status }));
}

const $ = (id) => document.getElementById(id);

function showApp() {
  $("app").classList.remove("hidden");
  $("logout").classList.remove("hidden");
  refresh();
}

async function refresh() {
  const listed = await API.listNotes();
  $("out").textContent = JSON.stringify(listed, null, 2);
}

$("register").onclick = async () => {
  const out = await API.register($("email").value, $("password").value);
  if (out.ok) showApp(); else alert(JSON.stringify(out));
};

$("login").onclick = async () => {
  const out = await API.login($("email").value, $("password").value);
  if (out.ok) showApp(); else alert(JSON.stringify(out));
};

$("logout").onclick = async () => {
  await API.logout();
  location.reload();
};

$("create").onclick = async () => {
  const out = await API.createNote($("body").value);
  if (out.ok) { $("body").value = ""; refresh(); } else alert(JSON.stringify(out));
};

// Resume an existing session on load.
API.me().then((me) => { if (me.ok) showApp(); });