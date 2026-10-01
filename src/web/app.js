"use strict";
// Every path is relative so the page works under Home Assistant's ingress prefix.
const framed = window.self !== window.top;
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private window */ } },
};
const state = { links: [], tag: null, text: "" };
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

function card(l) {
  const a = el("article");
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
  return a;
}

function render() {
  const list = document.getElementById("list");
  const q = state.text.toLowerCase();
  const shown = state.links.filter((l) =>
    (!state.tag || (l.tags || []).includes(state.tag)) &&
    (!q || [l.title, l.why, l.summary || "", (l.tags || []).join(" ")].join(" ").toLowerCase().includes(q)));
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
    const foot = document.getElementById("foot");
    foot.textContent = state.links.length + " finds" + (d.left_out && d.left_out.length ? ", " + d.left_out.length + " entries left out of the file" : "");
    render();
    store.set("finds.seen", JSON.stringify(state.links.map((l) => l.url).slice(0, 2000)));
  })
  .catch(() => { document.getElementById("list").replaceChildren(el("p", "empty", "Could not load the list.")); });
