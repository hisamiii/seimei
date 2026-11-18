# The watch's adb relay

The relay lets adb on the server reach the Wear OS watch's own adbd, with the watch's wifi off. Nothing can connect to a watch on its phone's bluetooth proxy or its own LTE, so the watch app connects back to the server instead.

```
adb connect 127.0.0.1:5590
adb -s 127.0.0.1:5590 shell echo ok
```

## How it works

Each adb connection wakes the watch, and the watch app carries its bytes.

1. **adb connects** to 127.0.0.1:5590 on the server (`[server] adb`; 0 turns the relay off, and `--dev` uses 5591).
2. **The server wakes the watch** with a push (`kind` adb, the connection's `id`) to every watch app that said it runs the relay. A watch app says so after each login, with POST /api/adb/ready, which marks its device.
3. **The watch app answers.** The push starts a foreground service, which logs in with the watch's key and opens GET /api/adb/ws?id=N over the app's usual link.
4. **Bytes go both ways as they are**, between adb's connection and adbd on the watch's 127.0.0.1:5555. adb's own login holds: the watch asks to allow this computer's key, as over wifi.
5. **It closes** when either side does, or after 10 minutes with no bytes. The server then runs `adb disconnect 127.0.0.1:5590`, so adb does not connect again and wake the watch by itself.

A connection waits 30 s for its watch. The server log says why one ended:

- `no watch with the relay can get a push`: no watch app has logged in since it got the relay, or its push token is gone.
- `no watch answered in 30 s`: the push did not arrive, or the watch could not log in. Its key refuses while the watch is locked.
- `closed (adbd is not listening on 5555)`: adbd listens on 5555 only after `adb tcpip 5555`, and only until the watch reboots.

## Costs

The watch holds nothing open for the relay: it is woken only when adb connects. While adb works, the service shows an "adb relay" notification.

- **Slow links.** A relay waits for its bytes to drain once 1 MB is queued, as the bluetooth proxy is slow.
- **A daily limit.** Wear OS runs this kind of service for at most 6 hours a day.

The code is `crates/server/src/adb.rs` on the server and `AdbRelay.kt` in the watch app.
