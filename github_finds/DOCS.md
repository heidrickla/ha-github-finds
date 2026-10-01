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

One JSON file, `links.json`, in the add-on's config folder (`/addon_configs/local_github_finds/` on the host, `/config` inside the add-on). The add-on only reads it, on every request, so a change shows on the next page load.

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

## Adding finds

The same binary edits the file over SSH from a workstation with `gh` logged in. It reads the repository's description, stars, language, last push and topics from the GitHub API.

```
github-finds add https://github.com/owner/repo --why "Why it was picked" --tags proxmox,rust
github-finds list
github-finds remove https://github.com/owner/repo
```

Defaults: `--host claude@192.0.2.10`, `--ssh-port 2222` (the Advanced SSH add-on, whose user has `sudo`), `--key ~/.ssh/ha_ed25519`. Without `--tags` the first four GitHub topics are used.
