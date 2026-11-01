"use strict";
// Every path is relative so the page works under Home Assistant's ingress prefix.
const framed = window.self !== window.top;
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private window */ } },
};
const state = { links: [], tag: null, text: "", leftOut: 0, discovery: "", canRemove: false };
const deletes = new Map();
// Links this browser has already shown; the first visit marks nothing new.
let seen = null;
try { seen = new Set(JSON.parse(store.get("finds.seen") || "null")); } catch { seen = null; }
const firstVisit = store.get("finds.seen") === null;

function theme(t) {
  document.documentElement.dataset.theme = t;
  document.getElementById("theme").textContent = t === "dark" ? "Light" : "Dark";
  store.set("finds.theme", t);
}
theme(store.get("finds.theme") || "dark");
document.getElementById("theme").addEventListener("click", () => {
  theme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
});

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

const SVG = "http://www.w3.org/2000/svg";
function trashIcon() {
  const s = document.createElementNS(SVG, "svg");
  const attrs = { viewBox: "0 0 24 24", width: "16", height: "16", fill: "none", stroke: "currentColor",
    "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" };
  Object.entries(attrs).forEach(([k, v]) => s.setAttribute(k, v));
  ["M4 7h16", "M9 7V4h6v3", "M6 7l1 13h10l1-13", "M10 11v6", "M14 11v6"].forEach((d) => {
    const p = document.createElementNS(SVG, "path");
    p.setAttribute("d", d);
    s.append(p);
  });
  return s;
}

function foot(error) {
  const f = document.getElementById("foot");
  const counts = state.links.length + " finds" + (state.leftOut ? ", " + state.leftOut + " entries left out of the file" : "");
  f.replaceChildren(counts + (state.discovery ? ". " + state.discovery : ""));
  if (error) f.append(el("p", "error", error));
}

// The first click arms the button for 4 s; a second click within that deletes the find.
function trash(l) {
  const b = el("button", "trash");
  b.type = "button";
  const deleting = deletes.get(l.url);
  const armed = deleting && deleting.until > Date.now();
  if (armed) b.classList.add("armed");
  b.replaceChildren(trashIcon(), ...(armed ? [el("span", "", "Delete?")] : []));
  b.setAttribute("aria-label", "Delete " + l.title + (armed ? "? Press again to confirm" : ""));
  b.title = armed ? "Delete?" : "Delete";
  b.disabled = deleting?.pending === true;
  b.addEventListener("click", () => {
    const current = deletes.get(l.url);
    if (current?.pending) return;
    if (!current || current.until <= Date.now()) {
      const entry = { until: Date.now() + 4000, pending: false };
      deletes.set(l.url, entry);
      setTimeout(() => {
        if (deletes.get(l.url) === entry && !entry.pending) {
          deletes.delete(l.url);
          render();
        }
      }, 4000);
      render();
      return;
    }
    current.pending = true;
    render();
    fetch("remove", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ url: l.url }) })
      .then((r) => {
        if (!r.ok) throw new Error(String(r.status));
        deletes.delete(l.url);
        state.links = state.links.filter((x) => x.url !== l.url);
        if (state.tag && !state.links.some((x) => (x.tags || []).includes(state.tag))) state.tag = null;
        foot();
        render();
      })
      .catch((e) => {
        deletes.delete(l.url);
        render();
        foot("Could not delete: " + (/^\d+$/.test(e.message) ? e.message : "no answer"));
      });
  });
  return b;
}

function card(l) {
  const a = el("article", state.canRemove ? "removable" : "");
  const title = el("a", "title", l.title);
  title.href = l.url;
  title.rel = "noopener noreferrer";
  // The ingress frame allows popups and not top navigation.
  if (framed) title.target = "_blank";
  a.append(title, el("p", "why", l.why));
  if (l.summary) a.append(el("p", "summary", l.summary));
  const meta = el("div", "meta");
  if (!firstVisit && seen && !seen.has(l.url)) meta.append(el("span", "new", "new"));
  if (l.language) meta.append(el("span", "", l.language));
  if (l.stars !== undefined) meta.append(el("span", "", "★ " + l.stars.toLocaleString()));
  if (l.pushed) meta.append(el("span", "", "pushed " + l.pushed));
  meta.append(el("span", "", "added " + l.added));
  a.append(meta);
  if (l.tags && l.tags.length) {
    const t = el("div", "tags");
    l.tags.forEach((x) => t.append(el("span", "", x)));
    a.append(t);
  }
  if (state.canRemove) a.append(trash(l));
  return a;
}

function render() {
  const list = document.getElementById("list");
  const q = state.text.toLowerCase();
  const shown = state.links.filter((l) =>
    (!state.tag || (l.tags || []).includes(state.tag)) &&
    (!q || [l.title, l.why, l.summary || "", (l.tags || []).join(" ")].join(" ").toLowerCase().includes(q)));
  document.getElementById("shown-count").textContent = String(shown.length);
  document.getElementById("total-count").textContent = state.tag || state.text
    ? `of ${state.links.length} finds shown` : `finds in your list`;
  list.replaceChildren(...(shown.length ? shown.map(card) : [el("p", "empty", state.links.length ? "Nothing matches." : "No finds yet.")]));
  const tags = document.getElementById("tags");
  const all = [...new Set(state.links.flatMap((l) => l.tags || []))].sort();
  tags.replaceChildren(...all.map((t) => {
    const c = el("button", "chip" + (t === state.tag ? " on" : ""), t);
    c.type = "button";
    c.addEventListener("click", () => { state.tag = state.tag === t ? null : t; render(); });
    return c;
  }));
}

document.getElementById("filter").addEventListener("input", (e) => { state.text = e.target.value; render(); });

fetch("links.json", { cache: "no-store" })
  .then((r) => r.json())
  .then((d) => {
    state.links = d.links || [];
    state.leftOut = (d.left_out || []).length;
    state.discovery = d.discovery || "";
    state.canRemove = d.can_remove === true;
    foot();
    render();
    store.set("finds.seen", JSON.stringify(state.links.map((l) => l.url).slice(0, 2000)));
  })
  .catch(() => { document.getElementById("list").replaceChildren(el("p", "empty", "Could not load the list.")); });
