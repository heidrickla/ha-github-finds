"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

test("armed delete survives filter and tag rerenders and sends once", async () => {
  const elements = new Map();
  function element(tag) {
    const node = {
      tag, children: [], listeners: {}, dataset: {}, attrs: {}, className: "", textContent: "",
      classList: { add() {}, remove() {} },
      append(...items) { this.children.push(...items); },
      replaceChildren(...items) { this.children = items; },
      addEventListener(name, fn) { this.listeners[name] = fn; },
      setAttribute(name, value) { this.attrs[name] = value; },
    };
    return node;
  }
  for (const id of ["theme", "foot", "list", "tags", "filter"]) elements.set(id, element("div"));
  const document = {
    documentElement: element("html"),
    getElementById(id) { return elements.get(id); },
    createElement: element,
    createElementNS(_ns, tag) { return element(tag); },
  };
  let removeCalls = 0;
  let finishRemove;
  const context = vm.createContext({
    window: { self: {}, top: {} }, document,
    localStorage: { getItem() { return null; }, setItem() {} },
    fetch(path) {
      if (path === "links.json") return Promise.resolve({ json: () => Promise.resolve({
        can_remove: true,
        links: [{ url: "https://github.com/o/r", title: "o/r", why: "why", added: "2026-10-01", tags: ["mcp"] }],
      }) });
      removeCalls++;
      return new Promise((resolve) => { finishRemove = resolve; });
    },
    setTimeout() { return 1; }, clearTimeout() {}, console,
  });
  vm.runInContext(fs.readFileSync(require("node:path").join(__dirname, "app.js"), "utf8"), context);
  await new Promise((resolve) => setImmediate(resolve));
  const button = () => elements.get("list").children[0].children.at(-1);
  button().listeners.click();
  elements.get("filter").listeners.input({ target: { value: "o/r" } });
  elements.get("tags").children[0].listeners.click();
  elements.get("tags").children[0].listeners.click();
  assert.match(button().attrs["aria-label"], /Press again/);
  button().listeners.click();
  assert.equal(removeCalls, 1);
  button().listeners.click();
  assert.equal(removeCalls, 1);
  finishRemove({ ok: true });
  await Promise.resolve();
  assert.equal(elements.get("list").children[0].className, "empty");
});
