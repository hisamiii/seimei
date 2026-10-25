# The author's own setup

These files run seimei on its author's laptop and are kept here for reference, not for installing: `deploy.sh` moves a production checkout (`~/.local/share/seimei-app`) to a commit and restarts seimei once it is idle, the two units are that checkout's systemd services (one is a cloudflared tunnel with the author's config), and the rest is niri, autologin and waybar glue for that machine. `notes/` holds design notes and review material.

To install seimei, follow the README: `seimei setup` writes the service for the binary you run.
