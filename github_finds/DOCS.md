# GitHub Finds

New GitHub projects for your Home Assistant setup on one page: name, why it was picked, the project's own description, language, stars, last push and tags, newest first. Once a day the add-on searches GitHub and adds a few new projects. Projects you add yourself stay on the page beside them.

## Setup

1. Install the add-on and start it. Nothing needs to be typed.
2. Open Finds in the sidebar. The first search runs two minutes after the add-on starts, and the page shows its picks after it finishes.
3. Optional: in the Configuration tab, change the topics, or set a model router to have a model judge each project.

The page's footer and the add-on's log say what discovery does with the current options.

## What it serves

| Path | Content |
|---|---|
| `/` | The page: a filter box, tag buttons, a theme toggle (dark by default, the choice kept in the browser). A find this browser has not shown before is marked new. |
| `/links.json` | `{"links": [...], "left_out": [...], "discovery": "..."}`: the valid finds, a note for each entry of the file that was left out, and the sentence saying what discovery does. |
| `/healthz` | `ok`, for Docker's health check. |

| Way in | Where |
|---|---|
| Sidebar | Finds, through ingress. Links open in a new tab. |
| Port 8091 | `http://<home assistant>:8091/` on the LAN: the same read-only page without a Home Assistant login. Clear the port in the Network section to keep the page to the sidebar. |

## Discovery

Once a day the add-on searches GitHub, one search per topic, and adds up to `per_day` new projects.

| Step | What happens |
|---|---|
| Search | One GitHub search per topic, unauthenticated, 7 s apart: created in the last 90 days, pushed in the last 30, at least 5 stars, archived repositories and forks left out, sorted by stars, 10 results. |
| Filter | Kept: a licence GitHub recognises and a description. Skipped: anything listed, judged in the last 180 days, or added by discovery before, so a removed pick is not added again. |
| Pick, no router | The most starred candidates across the topics. The reason shown is the topic, the stars and the creation date. |
| Pick, with a router | Up to 40 candidates a run, each sent to the model with a brief built from `interests` (or the topics), the finds you added yourself with their reasons, and the discovered finds you removed. The model answers pick, score 0 to 10 and why; picks scoring 7 or more are kept, best score first, then stars, and the model's reason is shown. |
| Add | Up to `per_day`, one per owner. Tags are the project's topics that match a search topic or a tag already in the list, up to four, filled to two from its other topics. A pick past the quota or a second from one owner waits for the next day. |

The first check runs two minutes after start, then every 30 minutes; a run starts when a day has passed since the last finished one. A run that could not reach the router, or whose every search failed, does not count as finished, so the next check tries again. Each run logs one line: who judged, searches, candidates, verdicts, picks, deferred, and the names added. State lives in `/data/discover.json` inside the add-on.

### A model router

Any server with an OpenAI-compatible chat completions API that the add-on can reach works: Ollama (`http://<host>:11434/v1`), llama.cpp's server, vLLM, LocalAI and similar. The request asks for a JSON-schema reply with thinking turned off; a small local model is enough. With `model` empty the add-on asks the router for its model list and takes the first one.

## Options

| Option | Default | Meaning |
|---|---|---|
| `discover` | `true` | Run the daily search. |
| `per_day` | `3` | The most finds one day adds, 1 to 10. |
| `topics` | `home-assistant`, `hacs`, `esphome`, `zigbee`, `matter`, `home-automation` | GitHub topics, one search each: lowercase letters, digits and `-`. |
| `router` | empty | Optional. An OpenAI-compatible base URL ending in `/v1`. Empty picks by stars. |
| `model` | empty | Optional. The model name sent to the router. Empty takes the first model the router lists. |
| `interests` | empty | Optional. A few sentences about your setup and what you build, for the model's brief. Empty uses the topics. |

Example, with a model on Ollama:

```yaml
discover: true
per_day: 3
topics:
  - home-assistant
  - esphome
  - proxmox
router: http://192.168.1.20:11434/v1
model: llama3.2:3b
interests: Home Assistant on a mini PC with Zigbee lights and ESPHome sensors; I write my own automations in Python.
```

## The list

One JSON file, `links.json`, in the add-on's config folder (`/addon_configs/<id>_github_finds/` on the host, `/config` inside the add-on; the File editor, Samba and SSH add-ons can reach it). The page reads it on every request, so a change shows on the next page load. Discovery adds to it.

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

One run now, inside the add-on's container; the paths default to the add-on's. `--dry-run` prints each verdict and writes nothing.

```
github-finds discover --dry-run
github-finds discover
```

### Adding finds from a workstation

The same binary, built on a workstation with Rust (`cargo install --git https://github.com/heidrickla/ha-github-finds github-finds`), edits the file over SSH with `gh` logged in for the GitHub API. It reads the repository's description, stars, language, last push and topics.

```
github-finds add https://github.com/owner/repo --why "Why it was picked" --tags esphome,rust --host root@homeassistant.local
github-finds list --host root@homeassistant.local
github-finds remove https://github.com/owner/repo --host root@homeassistant.local
```

| Flag | Variable | Default |
|---|---|---|
| `--host` | `GITHUB_FINDS_HOST` | none; required |
| `--ssh-port` | `GITHUB_FINDS_SSH_PORT` | `22` |
| `--key` | `GITHUB_FINDS_KEY` | ssh's own choice |
| `--dir` | `GITHUB_FINDS_DIR` | `/addon_configs/75df1782_github_finds`, the folder of this add-on installed from this repository |

The host is one of the SSH add-ons; a user other than root needs passwordless `sudo`. Without `--tags` the first four GitHub topics are used.

## Support

Issues: https://github.com/heidrickla/ha-github-finds/issues. Licence: MIT.
