# ha-github-finds

A Home Assistant add-on that serves a page of GitHub projects picked for the owner, finds new ones daily with a local model as judge, and the CLI that keeps its list.

| Path | What |
|---|---|
| `src/lib.rs` | The link model, the rules an entry must pass, and the server's routes. |
| `src/discover.rs` | Daily discovery: the search query, candidate filter, model brief and request, verdict parsing, selection and merge. |
| `src/main.rs` | `serve` for the add-on with the daily discovery thread; `discover` for one run now; `add`, `list`, `remove` over SSH from a workstation. |
| `src/web/` | The page, embedded in the binary. |
| `github_finds/` | The add-on: config, Dockerfile, run script, docs. |
| `tools/deploy.sh` | Builds the static binary and copies the add-on to the host's `/local_apps`. |

## Build and test

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
cargo deny check
cargo zigbuild --release --target x86_64-unknown-linux-musl
```

## Deploy

```
bash tools/deploy.sh
```

Then `ha apps install local_github_finds` the first time, `ha apps update local_github_finds` after a version change, or `ha apps rebuild local_github_finds` for a change at the same version.

Usage of the add-on and the list format: `github_finds/DOCS.md`.

## Licence

MIT. See `LICENSE`.
