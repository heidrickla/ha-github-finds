#!/bin/sh
# The links file is the add-on's config folder, /config inside the container.
set -eu
exec /usr/bin/github-finds serve --links /config/links.json --port 8091
