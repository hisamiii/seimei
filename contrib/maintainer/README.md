# Maintainer deploy

These files are one maintainer's production deploy of seimei from source, kept for reference and not for installing. To install seimei, follow [docs/install.md](../../docs/install.md), where `seimei setup` writes the service for the binary you run.

- `deploy.sh` moves a production checkout to a commit and restarts seimei once it is idle. [docs/architecture.md](../../docs/architecture.md#deploys) describes its steps.
- `seimei.service` is the systemd user service for that checkout.
- `seimei-tunnel.service` runs a cloudflared tunnel, as in [docs/reach.md](../../docs/reach.md).
- `seimei.kdl`, `seimei-autologin.conf` and `waybar-driving.sh` are niri, autologin and waybar glue for one desktop.

The units and the glue hold that deploy's own paths and config. Copy ideas from them, not the files.
