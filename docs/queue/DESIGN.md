# The agent queue: claude-queue inside seimei (design)

Status: for review, nothing built. Absorbs claude-queue (cqd, devgate, queue-prefix, bench, cq) into seimei as a general-purpose queue for agents on one machine, usable by people who are not this laptop's owner.

## Why

claude-queue makes several agents share one machine: their GPU/NPU jobs take turns, a bench gets a quiet machine, and nobody's job runs a 30 GB UMA laptop out of memory. Today it is a separate project with its own install, a fish function in front of `claude`, a Python daemon, and seimei as a client that guesses its paths. Three problems follow:

1. **Two cgroup roots, and benches miss one** (live, found while writing this). queue-prefix derives its root from the caller's own cgroup. seimei runs as an OpenRC service in `/1/seimei`, so its shikigami land in `/1/seimei/delegated/queue.slice/`; the user's sessions land in `/1/delegated/queue.slice/`. cqd freezes only `<its root>/queue.slice/queue-*.slice` (`bench_targets`, cqd.py:1393), its root being `/1/delegated`. A bench right now leaves every shikigami running (queue-dev, queue-audit, queue-connectors, queue-dictate). Device gating still works, since devgate and cqd's /proc scan match `/queue.slice/queue-<name>.slice/` anywhere.
2. **Placement depends on how a session was started.** A plain `claude` is queued only through the fish function. `claude --bg`, another shell, another agent (Codex, an ACP agent, aider) or a terminal tab started before the function existed all run outside the queue, and so does the GPU job they start.
3. **Nobody else can install it.** Paths, the exempt name "seimei", the desktop allowlist (niri, waybar, qs.py) and the fish wrapper are this laptop's.

The user asked: "why not absorb queue into seimei itself? and make it a general purpose agent queue".

## What exists (measured 2026-10-26)

| piece | size | what it does | verdict |
|---|---|---|---|
| `cqd.py` | 2,279 lines Python | scans /proc and cgroupfs every 1 s; leases, classes, ETAs, learned memory needs, desktop rehoming, bench freezing with verified GPU preemption, events, JSON socket | port to Rust, same behaviour, same protocol |
| `devgate.c` | 542 lines C | LD_PRELOAD shim on `open`/`openat` of device globs; asks cqd for a grant, falls back to its own scan | keep as C, ship it from the binary |
| `queue-prefix` | 30 lines sh | Claude Code's `CLAUDE_CODE_SHELL_PREFIX`: puts each command in `queue-<agent>.slice/run-<pid>.scope`, preloads devgate | becomes `seimei queue prefix` |
| `claude-queue`, `claude.fish` | 31 + 9 lines | set the agent name and env, then exec claude | replaced (see Placement) |
| `bench` | 306 lines bash | settle CPU and temperature, freeze through cqd, run, thaw; v1 freezing when cqd is down | becomes `seimei bench`, a cqd client as today |
| `cq` | 263 lines Python | status, watch, control, policy, history, ping, thaw-all | becomes `seimei queue <verb>` |
| `plugins/queue` | 923 lines Rust | the queue tools, the /proc fallback view, events to the dispatcher | stays, talks to the new daemon |
| tests | 1,500 lines Python and sh | drive cqd through its socket with a fake device in /dev/shm and throwaway `queue-cqt*` slices | the conformance suite for the port (see Migration) |

cqd here: 26 MB RSS, 5 min 44 s of CPU over 8 h 39 min (about 1.1% of a core). A queued command takes 12.7 ms against 1.0 ms for a bare `bash -c true` (the scope and the preload); a static `seimei` exec takes 0.7 ms, so a Rust prefix costs nothing extra.

## Shape

```
agent (any) ─ Bash cmd ─> seimei queue prefix ─> run-<pid>.scope in queue-<agent>.slice, LD_PRELOAD devgate.so
                                                     │ first device open: key=value lines
seimei serve (tools, app) <── JSON lines ──> seimei queue daemon  (same binary, its own user service)
seimei bench, seimei queue <verb> <── JSON lines ──┘  scans /proc + cgroupfs every 1 s, freezes, writes state
```

**One binary, two processes.** The daemon is `seimei queue daemon`, a second user service that `seimei setup` installs next to `seimei serve`. Not a task inside `seimei serve`, because:

- seimei restarts on every deploy (several a day) and on `seimei update`. The queue must not: a restart during a bench thaws everything (the thaw-on-stop rule), and waiters fall back to devgate's v1 scan for the gap.
- A panic in a route or a plugin must not take the scheduler, the frozen list or the leases with it.
- Someone may want the queue without the phone server (a workstation with three agents and one GPU).
- It keeps "never restart the queue from a queue slice" a property of one service file, as cqd's start_pre does today.

The daemon is a plain Rust module tree in a new crate `crates/queue` (`seimei-queue`): scheduler, scanner, cgroup and freeze code, the socket. `plugins/queue` keeps the tools and depends on it for the wire types. The rest of seimei does not link the scheduler's internals.

**The protocol stays.** cqd's two line protocols are kept byte for byte: the shim's `key=value` lines (`acquire`, `hello`, `wait`, `ka`, `grant`, `pass`) and the JSON ops (`status`, `watch`, `bench`, `history`, `events`, `policy`, `control`, `ping`). devgate.so v2, every running session's mapped shim, `bench`, `cq` and seimei's plugin keep working against the new daemon unchanged. Trust stays as cqd has it: SO_PEERCRED pid to cgroup; a process in an agent's queue slice reads and controls only its own leases, the rest may run every op.

## Resources

A resource is something agents take turns on. Four kinds, all in `[queue]` of config.toml:

```toml
[queue.resources.gpu]
devices = ["/dev/kfd", "/dev/dri/renderD*", "/dev/nvidia*"]
[queue.resources.npu]
devices = ["/dev/accel/accel*"]
turn = "gpu"                 # shares the gpu's turn (today's couple_gpu_npu)
[queue.resources.flasher]
devices = ["/dev/ttyACM*"]   # any path glob devgate sees opened
[queue.resources.emulator]
slots = 1                    # an explicit token: `seimei queue run --take emulator -- ./gradlew connectedCheck`
```

- **Devices** (gpu, npu, and any path glob). devgate already takes its globs from `CLAUDE_QUEUE_DEVICES`; the daemon hands each command the globs of every device resource, and a grant names the resource. `/dev/kfd` and `/dev/accel/*` are leased unless the desktop rules say otherwise; renderD alone is never "graphics" (Vulkan compute opens only renderD). Defaults: gpu and npu as above, coupled, as today. Several GPUs: one resource per card (`/dev/dri/renderD128` and `renderD129`, `nvidia0` and `nvidia1`), each its own turn.
- **Memory admission**, as cqd does it: need = p90 of a job key's learned peaks (anon + shmem + GTT/KFD buffers + half the file cache, x 1.15), else the declared `--mem`, else `mem_default_gb`; admit while need fits MemAvailable minus the reserve minus what running leases may still grow to; never force-grant past the reserve. On UMA (APUs) GPU memory is RAM, so this one gate covers both. Discrete VRAM: a later per-device gate from fdinfo `drm-resident-vram` against the card's total; out of scope for the first version.
- **CPU-heavy work** is shared, not queued: queue.slice gets a lower `cpu.weight` than the user's session (and benches freeze it), so a `cargo build -j32` in an agent never stutters the desktop and needs no wait. A declared `seimei queue run --take cpu -- ...` can still take an exclusive `cpu` token for jobs that must not overlap (two release builds fighting for LTO). The `nice -n 19` habit becomes unnecessary for queued agents.
- **User-defined tokens** (`slots = N`): taken explicitly with `--take`, for things devgate cannot see (an emulator, a licence seat, a rate-limited API). Matching processes by name is left out on purpose: exec interception is invasive and a wrong match blocks real work.

Policy (bench window, preemption, exempt agents, interactive daemons, desktop allowlist, memory numbers, event thresholds) keeps cqd's keys and defaults. Its file moves from `~/.config/claude-queue/policy.json` to seimei's state folder, `queue/policy.json`, because `queue_policy` writes it; config.toml holds only what the user sets by hand (resources, the cgroup root, a policy override).

The defaults stop naming this laptop: the exempt list becomes "seimei's dispatcher" by role (the daemon knows seimei's own slice name), the desktop allowlist keeps the generic entries (browsers, electron gpu-process, Xwayland, terminals, compositors) and loses qs.py, z13gui and friends, which move to this laptop's config.

## Placement

The hierarchy stays: `<root>/queue.slice/queue-<agent>.slice/run-<pid>.scope`, desktop apps rehomed to `<root>/app.slice/app-cq-<name>.scope`.

**One root, decided by the daemon.** The prefix never derives the root from its own cgroup again. The daemon picks it at start and writes it to `$XDG_RUNTIME_DIR/seimei/queue/root`; the prefix reads that file. Under systemd the root is the user manager's (`user@<uid>.service`) and scopes are made with `systemd-run --user --scope --slice=...` as today (writing into a systemd-managed tree by hand would fight systemd). Without systemd it is elogind's delegated session cgroup (`/<session>/delegated`), and the prefix makes the scope itself, as queue-prefix does. This fixes problem 1.

**seimei's own sessions** (dispatcher, shikigami on Claude Code) already get the prefix through the plugin's `env` hook; nothing changes except the prefix being `seimei queue prefix` and the name coming from seimei. ACP and Codex shikigami have no shell prefix hook: seimei starts the agent process itself inside `queue-<name>.slice/agent.scope` with devgate preloaded, so the whole agent is the unit (its commands inherit both). A bench then freezes the agent's own process too; that is the price of an agent without a prefix hook, and it only pauses its API calls.

**The user's own Claude Code sessions, without the fish function.** Claude Code reads `env` from `~/.claude/settings.json` for every session, however it was started (a terminal, `--bg`, an IDE). `seimei setup` offers to add one entry:

```json
{"env": {"CLAUDE_CODE_SHELL_PREFIX": "/home/you/.local/bin/seimei queue prefix"}}
```

With no `CLAUDE_QUEUE` set, the prefix names the agent after the claude process that runs it: `q<pid>`, where the pid is its parent's, and `~/.claude/sessions/<pid>.json` is that session in Claude Code's registry. So seimei shows the session's name and folder for every queued command, which today needs a scan of `/proc/*/environ`. A session seimei starts keeps the name seimei gives it (`CLAUDE_QUEUE` in its environment wins). This is the user's Claude Code config, so setup asks first, shows the line, and the guard keeps agents from changing it. To verify before building (V1): that a settings `env` entry reaches `CLAUDE_CODE_SHELL_PREFIX` (it is read from the process environment; settings `env` is applied at startup in current versions, but this needs a test on the installed version), and how a value with a space is split (else a one-line wrapper script).

**Other agents in a terminal** (Codex, aider, gemini): `seimei queue run --agent NAME -- codex` places the whole agent, as for ACP shikigami. A shell alias is the user's choice; seimei does not install shell functions.

**The daemon and seimei** are never in queue.slice. `seimei queue daemon` refuses to start inside one, and its service file starts it from the login session (OpenRC) or as its own unit (systemd), like cqd's start_pre and c55bfdd.

## Benches and freezing

Kept exactly, because each rule is a measured fact (claude-queue's DESIGN-scheduler.md section 5, cqd's V4 notes):

- freeze through `cgroup.freeze`, never `systemctl freeze`; a scope created under a frozen cgroup starts frozen; keep the slice loaded while frozen; a poller freezes agents that appear mid-bench;
- record a slice before freezing it; thaw on bench end, after `max_freeze_s` (3600), on daemon start, and in the service's stop hook (`seimei queue thaw-all`);
- a GPU holder is paused only after its GPU work drains (KFD cu_occupancy 0, SDMA and engine time flat for 1.5 s), else it is thawed at once and never paused again; never pause NPU users (unpinned buffers), unobservable jobs, exempt agents or benches;
- never `memory.reclaim` a cgroup that holds GPU/NPU fds.

`seimei bench [--settle ...] -- cmd` is today's `bench`: wait for its turn (it jumps the batch queue), freeze, settle CPU and temperature, run with its lease token, thaw, report contamination. With the one-root fix it freezes every agent, seimei's shikigami included.

## Without cgroup delegation

Some machines give the user no writable cgroup (no systemd user manager, elogind without `DelegateSessionCgroups=yes`, most containers). Then:

| feature | systemd user session | elogind, delegated | no delegation |
|---|---|---|---|
| GPU/NPU turns (devgate + daemon) | yes | yes | yes: holders are the command's process tree (`CQ_LEADER` and its descendants, pidfd against reuse) instead of its scope |
| memory admission and learned needs | per scope (memory.stat) | per scope | per process tree, from /proc smaps_rollup; coarser, sampled every 5 s |
| benches freeze agents | yes | yes | no: a bench waits for its turn and the CPU to settle, then runs and is marked "not isolated" |
| GPU preemption (pause a job) | yes | yes | no (SIGSTOP is not safe for GPU work; not offered) |
| desktop apps moved out | yes | yes | no: allowlisted desktop processes pass devgate, nothing moves |
| per-agent CPU weight | yes | yes | no |
| kill a job | `cgroup.kill` | `cgroup.kill` | the process group, SIGTERM then SIGKILL |

`seimei queue status` and the `queue` tool say which mode runs and what is missing, and setup prints the one line that turns delegation on where it can (elogind's logind.conf). Statically linked programs bypass devgate in every mode; the scan still sees their fds and counts them as holders after the fact, as today. Linux only.

## The queue tools

| tool | today | after |
|---|---|---|
| `queue` | cqd status, else a /proc snapshot | the same; status gains the mode (table above), resources by name, and the user's sessions named from the registry |
| `queue_control` | bump, hold, unhold, pause, resume, revoke (kill behind a card), park, unpark, admit, grants | the same actions; targets by resource too (`gpu`, `emulator`) |
| `queue_policy` | get or set cqd's policy | the same keys, plus resources read-only (they are config) |
| `queue_history` | past jobs | the same, with the resource |
| triggers `queue_free`, `queue_wait` | yes | the same, per resource optionally (`queue_wait` target `gpu:10`) |
| worker note `{launch}` | systemd-run or niri spawn | `seimei queue launch -- <app>`: starts a desktop app outside the queue on any init and compositor, so the note no longer depends on the machine |

The plugin's /proc fallback stays for the minutes the daemon restarts. The plugin's `wrap` (a session opened in a terminal stays queued) becomes the env entry instead of a `claude-queue` argv prefix.

## Migration on this laptop, with no downtime

Constraints: running Claude Code processes keep the environment they started with (`CLAUDE_CODE_SHELL_PREFIX=~/.local/lib/claude-queue/queue-prefix`, `CLAUDE_QUEUE_LIB=.../devgate.so`), the shim is mapped into running GPU jobs, waiters reconnect within 2 s or fall back to v1, and nothing may restart seimei or the queue from a queue slice.

0. **Now, independent of the rest:** fix the two-root bug in cqd (`bench_targets` and the frozen-slice sweep over every `*/delegated/queue.slice` under the session, or point seimei's prefix at the login session's root). Small, testable with the existing suite, and benches become clean again today.
1. **Port with the old tests as the judge.** The Python tests start `cqd.py --runtime-dir --state-dir --config-dir ...` and talk to its socket. The Rust daemon takes the same flags, so the same suite runs against both; the port is done when it passes unchanged. New Rust tests cover resources, the one-root rule and no-delegation mode.
2. **Shadow.** Run `seimei queue daemon --shadow` beside cqd for a few days: it scans and classifies, binds no socket, freezes nothing, and logs every decision that differs from cqd's state.json and events (cqd's own shadow.jsonl did this for v1 to v2).
3. **Keep the old paths alive.** Install keeps writing `~/.local/lib/claude-queue/devgate.so` and `queue-prefix` by rename-into-place, so every running session's baked environment stays valid; queue-prefix becomes a two-line `exec seimei queue prefix "$1"`.
4. **Cut over at a quiet moment.** A script, started from the login session (never a queue slice), waits until no bench runs and nothing is frozen, stops cqd (its stop hook thaws nothing, since nothing is frozen), and starts the new service on the same socket path, `$XDG_RUNTIME_DIR/claude-queue/cqd.sock`. It adopts holders by fd scan and waiter positions from state.json, which cqd already does after a crash. Holders keep their fds throughout; waiters reconnect with the same `req` and keep their places, or fall back to v1 for the gap, which serializes safely. This is the path every cqd restart already takes (the last on 2026-10-25, c55bfdd).
5. **Rollback** is the same step reversed: stop the new service, start cqd. The state file format stays readable by both until cqd is retired.
6. **Placement next.** After V1 passes, setup adds the settings `env` entry; new sessions get `seimei queue prefix` and the one root. The fish function goes once no shell still has the old function loaded (open terminals keep it until they close; harmless, both prefixes land in the same root after step 0).
7. **Paths last.** Move to `$XDG_RUNTIME_DIR/seimei/queue.sock` and `~/.local/lib/seimei/devgate.so` with the old paths as symlinks until no process maps the old shim (`grep -l claude-queue/devgate.so /proc/*/maps`). Then archive the claude-queue repo with a pointer here.

## Distribution

- **devgate.so** is built by `build.rs` from `crates/queue/devgate.c` with the system C compiler and embedded in the binary; `seimei setup` (and the daemon at start) writes it to `~/.local/lib/seimei/devgate.so` by rename-into-place when it differs. The release binary is static musl, but the shim must load into glibc programs: release.sh builds it for x86_64 glibc separately (it needs only `dlsym`, `open`, `openat` and socket calls; built against glibc 2.34 or later because `dlsym` moved into libc there). A musl distribution needs a musl build of the shim (open question 4).
- Requirements in docs/install.md: cgroup v2, and for the full feature set a systemd user session or elogind with delegated session cgroups.

## Open questions for the user

1. Daemon as its own service of the same binary (recommended above) or a task inside `seimei serve`?
2. May setup write the `env` entry into `~/.claude/settings.json` (after asking), so every Claude Code session is queued without the fish function?
3. CPU: lower `cpu.weight` for agent slices by default, with an opt-in exclusive `cpu` token, instead of queueing builds?
4. Ship a glibc devgate.so with releases (works on most distributions), or build it at setup when a C compiler is there?
5. One queue per user only: sharing a GPU between Unix users needs a root daemon and is out of scope. Fine?

## To verify before building

- V1: settings `env` reaches `CLAUDE_CODE_SHELL_PREFIX` in the installed Claude Code, and how it splits a value with a space.
- V2: the shim built against glibc 2.34 loads in the GPU programs used here (llama.cpp, torch, ollama, realesrgan) and in Vesktop and Firefox when they inherit it.
- V3: per-process-tree memory sampling (no delegation) tracks a ROCm job's peaks within 10% of memory.stat.
- V4: two agents' `cargo build -j32` with queue.slice at `cpu.weight` 20 keep niri frame times flat.
