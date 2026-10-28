# Reach seimei from your phone

seimei listens on 127.0.0.1 only, so the phone needs HTTPS in front of it. Tailscale needs no domain, and a Cloudflare tunnel gives a name on your own domain.

HTTPS is required because the login cookie is Secure, and passkeys and notifications work only on https. seimei answers only requests for its own address, `[server] origin` in config.toml. Whatever sits in front must pass the Host header through unchanged, and both recipes below do.

Each recipe ends with an address. Put it in config.toml as the origin, then restart seimei. `seimei setup` asks for it on the first run.

```toml
[server]
origin = "https://box.tail1234.ts.net"
```

Then log in on the phone at that address, or pair the app with `seimei pair`. To restart, run `systemctl --user restart seimei`, or `rc-service --user seimei restart` on OpenRC.

## Tailscale: no domain needed

Your phone reaches seimei over your tailnet, and nothing is open to the internet. The origin is the server's `ts.net` address.

1. Install Tailscale on the server and on the phone, and log both into the same tailnet.
2. On the server, run `tailscale serve --bg 8417`, with your port if you changed it. The first time, it prints a link to turn on HTTPS for the tailnet. Open it, approve, and run the command again.
3. `tailscale serve status` shows the address, `https://<machine>.<tailnet>.ts.net`. That is the origin. setup finds it on its own when Tailscale runs.

The serve setting survives reboots. `tailscale serve --https=443 off` removes it.

## Cloudflare Tunnel: your own domain

Your phone reaches seimei from anywhere, at a name on a domain you have on Cloudflare. Anyone can load the login page, so the password and passkeys keep them out, see [security.md](security.md).

To try it first without an account, run `cloudflared tunnel --url http://localhost:8417`. It prints a `https://<random>.trycloudflare.com` address to use as the origin. The address changes every time cloudflared starts, and a passkey works only on the address it was made on. So this is for a trial, not for every day.

For a fixed name, replace `seimei.example.com` with yours:

1. Run `cloudflared tunnel login`, then pick the domain.
2. Run `cloudflared tunnel create seimei`. It prints the tunnel id and writes its credentials to `~/.cloudflared/<id>.json`.
3. Run `cloudflared tunnel route dns seimei seimei.example.com`.
4. Write `~/.cloudflared/seimei.yml`:

   ```yaml
   tunnel: seimei
   credentials-file: /home/you/.cloudflared/<id>.json
   ingress:
     - hostname: seimei.example.com
       service: http://localhost:8417
     - service: http_status:404
   ```

5. Run `cloudflared tunnel --config ~/.cloudflared/seimei.yml run`. To keep it running, make a user service with that command. `contrib/maintainer/seimei-tunnel.service` is a systemd example.

The origin is `https://seimei.example.com`.

## Other ways

A reverse proxy with its own certificate, such as Caddy or nginx, works too. It must keep the Host header and must not buffer responses, because the page follows a live event stream.
