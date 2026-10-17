#!/bin/sh
# The links file is the add-on's config folder, /config inside the container; discovery keeps its
# state in /data.
set -eu
exec /usr/bin/github-finds serve --links /config/links.json --port 8091 --options /data/options.json --state /data/discover.json
