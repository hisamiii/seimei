# Install

Install seimei from a signed release, or build it from source, then run `seimei setup`. Setup writes the config, sets the password and installs the user service.

## Requirements

seimei needs Linux on x86_64 and a logged-in Claude Code 2.1 or later. The phone needs HTTPS in front of seimei.

- Claude Code is installed and logged in as the user seimei runs as: run `claude`, then `/login`. seimei drives that installation with its login and settings, so it needs no API key.
- HTTPS in front of seimei lets the phone reach it. [reach.md](reach.md) has recipes for Tailscale and a Cloudflare tunnel. Without it, seimei works only in the server's own browser.
- The phone app needs Android 11 or later. An iPhone uses the web page, added to the Home Screen for notifications.
- Each plugin needs its own programs, such as a supported window manager, playerctl or the browser extension. See [plugins.md](plugins.md).
- A build from source needs Rust 1.86 or later (edition 2024) and a C compiler. openssl is vendored.
- seimei runs as a systemd or OpenRC user service. Without either, start `seimei serve` at login yourself.

## From a release

A release is one signed binary, `seimei-x86_64-linux`. Check its signature, put it on your PATH and run `seimei setup`.

1. Download `seimei-x86_64-linux` and `seimei-x86_64-linux.minisig` from the release page.
2. Check the signature with `minisign -V -p minisign.pub -m seimei-x86_64-linux`. The key is `contrib/minisign.pub` in this repo.
3. Put the binary where it stays, for example `~/.local/bin/seimei`, and make it executable.
4. Run `seimei setup`. It checks Claude Code and asks how the phone reaches the server, your plan, the terminal and the plugins. Then it writes `~/.config/seimei/config.toml`, sets the password, writes the user service and offers to start it.
5. Open the address setup prints and log in. From the menu, add a passkey, pair the phone app and turn on notifications. `seimei pair` prints the pairing code.

Running `seimei setup` again leaves what is there alone.

`seimei update` installs a newer release over the binary and keeps the old one as `seimei.old`. It checks the minisign signature first. A running server restarts on the new binary once no turn runs. `seimei update --check` only says whether there is a newer release.

## From source

Build the `seimei` package and run setup from the build.

```
cargo build --release -p seimei
target/release/seimei setup
```

A build from source does not update itself. Pull and build again.

## The phone app

The release page has `seimei.apk`, the foss build. On first run it asks for seimei's address, and `seimei pair` gives the code to pair it.

The foss build gets notifications through a UnifiedPush distributor, such as ntfy, when the phone has one. Otherwise it keeps a background connection to seimei.

### Your own build with Firebase (FCM)

FCM delivers through Google Play services, with no distributor app and no background connection. The server can push only to an FCM build made for its own Firebase project, so you build that app yourself.

1. In the Firebase console, make a project and add an Android app with the package name `com.hisamiowo.seimei`. Save its `google-services.json` as `~/.local/share/seimei/firebase/google-services.json`.
2. In the project's settings, under Service accounts, generate a private key. Save it as `~/.local/share/seimei/firebase/service-account.json` and `chmod 600` it. seimei sends with this key, and the default `deny_paths` keeps Claude Code sessions out of `firebase/`.
3. Install JDK 21 and the Android SDK, and point `ANDROID_HOME` at the SDK. The command-line tools are enough.
4. Run `android/publish.sh minor`. With the Firebase config in place it builds the fcm flavor and puts the APK where the menu's Phone app offers it.
5. Restart seimei so it picks up the service account.

`publish.sh` signs the APK with a key it makes once in `~/.local/share/seimei/signing/`. Back that folder up, because updates install only over the same key. The release APK has another key, so uninstall it from the phone before your build installs.

## Remove

Stop and disable the service, then delete the binary, `~/.config/seimei/`, `~/.local/share/seimei/` and the service file setup wrote. Claude Code's own sessions and transcripts stay where Claude Code keeps them.

- systemd: `systemctl --user disable --now seimei`. The service file is `~/.config/systemd/user/seimei.service`.
- OpenRC: `rc-update --user del seimei default`, then `rc-service --user seimei stop`. The service file is `~/.config/rc/init.d/seimei`.
