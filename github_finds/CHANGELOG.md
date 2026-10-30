# Changelog

## 0.4.0

- Each card in the sidebar panel has a trashcan in its bottom-right corner: the first press arms it, a second within 4 s deletes the find from the list and keeps discovery from suggesting it again. The LAN port stays read-only and shows no trashcan.

## 0.3.1

- The image cross-compiles the binary on the build machine's own platform for the target, so an aarch64 image built on an x86 machine no longer compiles under emulation. On a Raspberry Pi the build is native as before.

## 0.3.0

Backward-incompatible: `router` and `model` no longer have defaults, and the default `topics` changed. An install that relied on the old defaults sets `router`, `model` and `topics` in the options to keep its behaviour.

- The add-on installs from the repository: the Dockerfile builds the binary, for amd64 and aarch64.
- With no `router` set, daily discovery adds the most starred new projects for each topic, so the page fills with no option typed.
- An empty `model` takes the first model the router lists.
- `interests`: the owner's setup in their own words, for the model's brief; empty means the topics.
- The page's footer and the log say what discovery does with the current options.
- Default topics: `home-assistant`, `hacs`, `esphome`, `zigbee`, `matter`, `home-automation`.
- Docker's health check replaces the Supervisor watchdog; an AppArmor profile; the config folder is mapped as `app_config`.
- The CLI has no default host: `--host` or `GITHUB_FINDS_HOST`, with `--key`, `--ssh-port` and `--dir` or their variables; root on the host needs no sudo.

## 0.2.2

- rustls-webpki 0.103.16 and webpki-roots 1.0.10.

## 0.2.1

- A find's tags are its topics that match a search topic or a tag in the list, filled to two from its other topics.
- One pick per owner a day; the others are judged again the next day.
- `discover` errors and the HTTP status mapping live in the library.

## 0.2.0

- Daily discovery: a GitHub search per topic for licensed, active projects created in the last 90 days, each new one judged by a local model through an OpenAI-compatible router, and up to `per_day` picks added with the model's reason as `why`.
- The model's brief is built from the finds added by hand and the discovered finds that were removed.
- Options `discover`, `per_day`, `model`, `topics`, `router`.
- `github-finds discover [--dry-run]` runs one search now.
- The add-on's config folder is writable, for the picks.

## 0.1.0

- The page: finds newest first, filter box, tag buttons, dark theme by default with a toggle, new finds marked per browser.
- `/links.json` with the finds and the entries left out; `/healthz`.
- Sidebar panel through ingress and port 8091 on the LAN.
- `add`, `list` and `remove` keep the list over SSH, taking a repository's details from the GitHub API.
