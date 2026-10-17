# GitHub Finds

GitHub projects picked for you, on one page: name, why it was picked, the project's own description, language, stars, last push and tags, newest first.

## What it serves

| Path | Content |
|---|---|
| `/` | The page: a filter box, tag buttons, a theme toggle (dark by default, the choice kept in the browser). A find this browser has not shown before is marked new. |
| `/links.json` | `{"links": [...], "left_out": [...]}`: the valid finds and a note for each entry of the file that was left out. |
| `/healthz` | `ok`, for the Supervisor watchdog. |

| Way in | Where |
|---|---|
| Sidebar | Home Assistant, Finds (ingress). Links open in a new tab. |
| Port | `http://<home assistant>:8091/` on the LAN. No login of its own. |

## The list

One JSON file, `links.json`, in the add-on's config folder (`/addon_configs/local_github_finds/` on the host, `/config` inside the add-on). The page reads it on every request, so a change shows on the next page load. Discovery appends to it; the CLI below edits it.

| Field | Required | Meaning |
|---|---|---|
| `url` | yes | A `https://github.com/` page. |
| `title` | yes | 1 to 120 characters, usually `owner/repo`. |
| `why` | yes | Why it was picked, 1 to 500 characters. |
| `added` | yes | `YYYY-MM-DD`. |
| `summary` | no | The project's description. |
| `tags` | no | Up to 8 of lowercase letters, digits and `-`. |
| `stars`, `language`, `pushed` | no | From the GitHub API; `pushed` is `YYYY-MM-DD`. |

An entry that breaks a rule is left out of the page and listed in `left_out` and the log; the rest still show.

## Discovery

Once a day the add-on searches GitHub and adds up to `per_day` projects a local model picks.

| Step | What happens |
|---|---|
| Search | One GitHub search per topic, unauthenticated, 7 s apart: created in the last 90 days, pushed in the last 30, at least 5 stars, not archived, not a fork, sorted by stars, 10 results. |
| Filter | Kept: a licence GitHub recognises and a description. Skipped: anything listed, judged in the last 180 days, or added by discovery before (so a removed pick is not added again). |
| Judge | Up to 40 candidates a run, each sent to `model` on the router with a brief built from the finds added by hand and the discovered finds that were removed. The model answers pick, score 0 to 10 and why. |
| Add | Picks scoring 7 or more, best score first, then stars; up to `per_day`. The model's reason is the find's `why`, the first four topics its tags. A pick past the day's quota is judged again the next day. |

The first check runs two minutes after start, then every 30 minutes; a run starts when a day has passed since the last finished one. A run that could not reach the router, or whose every search failed, does not count as finished, so the next check tries again. The verdicts already in are kept and not asked again. Each run logs one line: searches, candidates, verdicts, picks, deferred, and the names added.

State lives in `/data/discover.json` inside the add-on.

| Option | Default | Meaning |
|---|---|---|
| `discover` | `true` | Run the daily search. |
| `per_day` | `3` | The most finds one day adds, 1 to 10. |
| `model` | `bonsai` | The model name sent to the router. |
| `topics` | `home-assistant`, `hacs`, `unifi`, `proxmox`, `esphome`, `zigbee`, `mcp`, `bitnet` | GitHub topics, one search each. |
| `router` | Voice Bridge, `http://local-voice-bridge:8789/v1` | Any OpenAI-compatible base URL ending in `/v1`. |

One run now, inside the add-on (`--dry-run` prints each verdict and writes nothing):

```
docker exec addon_local_github_finds github-finds discover --dry-run
docker exec addon_local_github_finds github-finds discover
```

## Adding finds by hand

The same binary edits the file over SSH from a workstation with `gh` logged in. It reads the repository's description, stars, language, last push and topics from the GitHub API.

```
github-finds add https://github.com/owner/repo --why "Why it was picked" --tags proxmox,rust
github-finds list
github-finds remove https://github.com/owner/repo
```

Defaults: `--host root@192.0.2.10`, `--ssh-port 22` (the Advanced SSH add-on), `--key ~/.ssh/ha_ed25519`. Without `--tags` the first four GitHub topics are used.
