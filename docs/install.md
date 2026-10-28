# Install

## Requirements

- Linux on x86_64, with systemd or OpenRC user services (without either, start `seimei serve` at login yourself).
- Claude Code 2.1 or later, installed and logged in as the user seimei runs as (`claude`, then `/login`). seimei drives that installation with its login and settings; it does not need an API key.
- HTTPS in front of seimei for the phone: [reach.md](reach.md) has recipes for Tailscale and a Cloudflare tunnel. Without it, seimei works in this machine's browser only.
- For the phone app: Android 11 or later. An iPhone uses the web page (add it to the Home Screen for notifications).
- Each plugin needs its own programs (a supported window manager, playerctl, the browser extension, ...): see [plugins.md](plugins.md).
- To build from source: Rust 1.86 or later (edition 2024) and a C compiler (openssl is vendored).

## From a release

1. Download `seimei-x86_64-linux` and `seimei-x86_64-linux.minisig` from the release page, and check the signature: `minisign -V -p minisign.pub -m seimei-x86_64-linux` (`contrib/minisign.pub` in this repo).
2. Put it where it stays, for example `~/.local/bin/seimei`, and make it executable.
3. Run `seimei setup`. It checks Claude Code, asks how the phone reaches this machine, your plan, the terminal and the plugins, writes `~/.config/seimei/config.toml`, sets the password, writes the user service and offers to start it. Running it again leaves what is there alone.
4. Open the address it prints and log in. From the menu: add a passkey, pair the phone app (`seimei pair` prints the code), turn on notifications.

`seimei update` installs a newer release over the binary (it checks the minisign signature first and keeps the old one as `seimei.old`); a running server restarts on it once no turn runs. `seimei update --check` only says whether there is one.

## From source

```
cargo build --release -p seimei
target/release/seimei setup
```

A build from source does not update itself; pull and build again.

## The phone app

The release page has `seimei.apk`, the foss build: notifications come through a UnifiedPush distributor (ntfy, for example) when the phone has one, else through a background connection to seimei. On first run it asks for seimei's address; pair it with `seimei pair`.

### Your own build with Firebase (FCM)

FCM delivers through Google Play services, with no distributor app and no background connection. The server can only push to an FCM build made for its own Firebase project, so this one you build yourself:

1. In the Firebase console, make a project and add an Android app with the package name `com.hisamiowo.seimei`. Save its `google-services.json` as `~/.local/share/seimei/firebase/google-services.json`.
2. In the project's settings, Service accounts, generate a private key and save it as `~/.local/share/seimei/firebase/service-account.json` (`chmod 600`). seimei sends with it; the guard keeps agents out of `firebase/`.
3. Install JDK 21 and the Android SDK (command-line tools are enough), and point `ANDROID_HOME` at it.
4. Run `android/publish.sh minor`. With the Firebase config there it builds the fcm flavor, signs it with a key it makes once in `~/.local/share/seimei/signing/` (back that folder up: updates only install over the same key), and puts the APK where the menu's Phone app offers it. Restart seimei so it picks up the service account.

The release APK is signed with another key, so the phone has to uninstall it before your build installs.

## Remove

Stop and disable the service (`systemctl --user disable --now seimei`, or `rc-update --user del seimei default` and `rc-service --user seimei stop`), then delete the binary, `~/.config/seimei/`, `~/.local/share/seimei/` and the service file setup wrote (`~/.config/systemd/user/seimei.service` or `~/.config/rc/init.d/seimei`). Claude Code's own sessions and transcripts stay where Claude Code keeps them.
