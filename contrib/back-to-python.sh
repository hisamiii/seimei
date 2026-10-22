#!/bin/sh
# back-to-python.sh: run seimei on the Python server again, on the same state files, and keep deploys on Python.
# Back to Rust: rm ~/.local/share/seimei/deploy/python, then ./deploy.sh.
set -eu
app=$(dirname "$(dirname "$(readlink -f "$0")")")
state=${SEIMEI_STATE:-$HOME/.local/share/seimei}
mkdir -p "$state/deploy"
touch "$state/deploy/python"
(cd "$app" && uv sync -q --frozen --no-dev)
cp --remove-destination "$app/contrib/seimei-python.service" "$HOME/.config/systemd/user/seimei.service"
systemctl --user daemon-reload
echo "seimei restarts on Python (deploys stay on Python while $state/deploy/python exists)"
systemctl --user restart seimei
