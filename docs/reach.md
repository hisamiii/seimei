# Reach seimei from your phone

seimei listens on 127.0.0.1 only. The phone needs HTTPS in front of it: the login cookie is Secure, and passkeys and notifications work only on https. seimei answers only requests for its own address (`[server] origin` in config.toml), so whatever sits in front must pass the Host header through unchanged; both recipes below do.

Each recipe ends with an address. Put it in config.toml as the origin (`seimei setup` asks for it on the first run), then restart seimei (`systemctl --user restart seimei`, or `rc-service --user seimei restart` on OpenRC). Then log in on the phone at that address, or pair the app with `seimei pair`.

```toml
[server]
origin = "https://box.tail1234.ts.net"
```

## Tailscale: no domain needed

Your phone reaches seimei over your tailnet, and nothing is open to the internet.

1. Install Tailscale on the server and on the phone, and log both into the same tailnet.
2. On the server: `tailscale serve --bg 8417` (your port if you changed it). The first time, it prints a link to turn on HTTPS for the tailnet: open it, approve, and run the command again.
3. `tailscale serve status` shows the address: `https://<machine>.<tailnet>.ts.net`. That is the origin. setup finds it on its own when Tailscale runs.

The serve setting survives reboots. `tailscale serve --https=443 off` removes it.

## Cloudflare Tunnel: your own domain

Your phone reaches seimei from anywhere, at a name on a domain you have on Cloudflare. Anyone can load the login page, so the password and passkeys are what keeps them out (see [security.md](security.md)).

To try it first, without an account: `cloudflared tunnel --url http://localhost:8417` prints a `https://<random>.trycloudflare.com` address. Use it as the origin. The address changes every time cloudflared starts, and a passkey only works for the address it was made on, so this is for trying seimei out, not for every day.

For a fixed name (replace `seimei.example.com` with yours):

1. `cloudflared tunnel login`, then pick the domain.
2. `cloudflared tunnel create seimei` (it prints the tunnel id and writes its credentials to `~/.cloudflared/<id>.json`).
3. `cloudflared tunnel route dns seimei seimei.example.com`
4. Write `~/.cloudflared/seimei.yml`:

   ```yaml
   tunnel: seimei
   credentials-file: /home/you/.cloudflared/<id>.json
   ingress:
     - hostname: seimei.example.com
       service: http://localhost:8417
     - service: http_status:404
   ```

5. Run it: `cloudflared tunnel --config ~/.cloudflared/seimei.yml run`. To keep it running, make a user service with that command (`contrib/maintainer/seimei-tunnel.service` is a systemd example).

The origin is `https://seimei.example.com`.

## Other ways

A reverse proxy with its own certificate (Caddy, nginx) works too if it keeps the Host header and does not buffer responses: the page follows a live event stream.
