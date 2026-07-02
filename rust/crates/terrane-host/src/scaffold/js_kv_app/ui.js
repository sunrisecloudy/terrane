/* EDIT GUIDE: edit the REPLACE-marked functions (renderItem, refresh, the
   submit handler). Keep the KEEP-marked helpers — they already handle loading,
   errors, and safe DOM building. style.css already styles .card .btn .badge
   .list-item .empty-state .status .grid; reuse those classes. */

/* KEEP: backend bridge. Positional string args only — never pass an array. */
async function invoke(verb, ...args) {
  setStatus("Working…", "loading");
  try {
    var out = await window.terrane.invoke(verb, ...args.map(String));
    setStatus("", "ok");
    return out;
  } catch (err) {
    setStatus(String(err), "error");
    throw err;
  }
}

/* KEEP: JSON.parse with a fallback for empty or malformed replies. */
function parseJsonOr(text, fallback) {
  try { return JSON.parse(text); } catch (err) { return fallback; }
}

/* KEEP: small DOM builder — safer than innerHTML for user text. */
function el(tag, className, text) {
  var node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

/* KEEP: status line (loading / error / ok). */
var statusTimer = null;
function setStatus(message, kind) {
  var box = document.getElementById("status");
  if (statusTimer) { clearTimeout(statusTimer); statusTimer = null; }
  if (!message) {
    box.hidden = true;
    box.className = "status";
    box.textContent = "";
    return;
  }
  box.hidden = false;
  box.className = "status " + (kind || "");
  box.textContent = message;
  if (kind === "ok") {
    statusTimer = setTimeout(function () { setStatus("", ""); }, 2000);
  }
}

/* REPLACE: render one item. `item` is one object from the "list" verb's JSON
   array. Return a DOM node (an li). */
function renderItem(item) {
  var li = el("li", "list-item");
  var body = el("div", "item-body");
  body.appendChild(el("div", "item-title", item.text || item.id));
  body.appendChild(el("div", "item-meta muted", item.id));
  li.appendChild(body);
  var actions = el("div", "item-actions");
  var del = el("button", "btn btn-ghost btn-sm", "Remove");
  del.type = "button";
  del.setAttribute("data-action", "remove");
  del.setAttribute("data-id", item.id);
  actions.appendChild(del);
  li.appendChild(actions);
  return li;
}

/* REPLACE: load and render the main view. */
async function refresh() {
  var items = parseJsonOr(await invoke("list"), []);
  var list = document.getElementById("list");
  var empty = document.getElementById("empty");
  list.textContent = "";
  for (var i = 0; i < items.length; i++) {
    list.appendChild(renderItem(items[i]));
  }
  empty.hidden = items.length > 0;
}

/* REPLACE: primary input — send the text to the backend, then refresh. */
document.getElementById("input-form").addEventListener("submit", async function (e) {
  e.preventDefault();
  var input = document.getElementById("main-input");
  var text = input.value.trim();
  if (!text) return;
  await invoke("add", text);
  input.value = "";
  await refresh();
});

/* KEEP: one delegated listener for per-row actions ([data-action] buttons). */
document.getElementById("list").addEventListener("click", async function (e) {
  var btn = e.target.closest("[data-action]");
  if (!btn) return;
  if (btn.getAttribute("data-action") === "remove") {
    await invoke("remove", btn.getAttribute("data-id"));
    await refresh();
  }
  /* ADD: more row actions here. */
});

refresh();
