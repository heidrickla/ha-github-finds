# Changelog

## 0.2.1

- A find's tags are its topics that match a search topic or a tag in the list, filled to two from its other topics.
- One pick per owner a day; the others are judged again the next day.
- `discover` errors and the HTTP status mapping live in the library.

## 0.2.0

- Daily discovery: a GitHub search per topic for licensed, active projects created in the last 90 days, each new one judged by a local model through the Voice Bridge router, and up to `per_day` picks added with the model's reason as `why`.
- The model's brief is built from the finds added by hand and the discovered finds that were removed.
- Options `discover`, `per_day`, `model`, `topics`, `router`.
- `github-finds discover [--dry-run]` runs one search now.
- The add-on's config folder is writable, for the picks.

## 0.1.0

- The page: finds newest first, filter box, tag buttons, dark theme by default with a toggle, new finds marked per browser.
- `/links.json` with the finds and the entries left out; `/healthz`.
- Sidebar panel through ingress and port 8091 on the LAN.
- `add`, `list` and `remove` keep the list over SSH, taking a repository's details from the GitHub API.
