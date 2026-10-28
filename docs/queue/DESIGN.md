# The agent queue: claude-queue inside seimei (design)

Status: the user's answers are in (Decisions). Built: the port (`crates/queue`, Migration step 1, passing claude-queue's socket suite, 150 checks, run without systemd by its new tests/cg.py) and the shadow inside serve (step 2). Next, before the cutover: the restart rules (persistence, the keeper). Absorbs claude-queue (cqd, devgate, queue-prefix, bench, cq) into seimei as a general-purpose queue for agents on one machine, usable by people who are not this laptop's owner.

## Why

claude-queue makes several agents share one machine: their GPU/NPU jobs take turns, a bench gets a quiet machine, and nobody's job runs a 30 GB UMA laptop out of memory. Today it is a separate project with its own install, a fish function in front of `claude`, a Python daemon, and seimei as a client that guesses its paths. Three problems follow:

1. **Two cgroup roots, and benches miss one** (found while writing this; fixed in step 0). queue-prefix derives its root from the caller's own cgroup. seimei runs as an OpenRC service in `/1/seimei`, so its shikigami land in `/1/seimei/delegated/queue.slice/`; the user's sessions land in `/1/delegated/queue.slice/`. cqd freezes only `<its root>/queue.slice/queue-*.slice` (`bench_targets`, cqd.py:1393), its root being `/1/delegated`. A bench right now leaves every shikigami running (queue-dev, queue-audit, queue-connectors, queue-dictate). Device gating still works, since devgate and cqd's /proc scan match `/queue.slice/queue-<name>.slice/` anywhere.
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
| `plugins/queue` | 923 lines Rust | the queue tools, the /proc fallback view, events to the dispatcher | stays, talks to the scheduler |
| tests | 1,500 lines Python and sh | drive cqd through its socket with a fake device in /dev/shm and throwaway `queue-cqt*` slices | the conformance suite for the port (see Migration) |

cqd here: 26 MB RSS, 5 min 44 s of CPU over 8 h 39 min (about 1.1% of a core). A queued command takes 12.7 ms against 1.0 ms for a bare `bash -c true` (the scope and the preload); a static `seimei` exec takes 0.7 ms, so a Rust prefix costs nothing extra.

## Shape

```
agent (any) ─ Bash cmd ─> seimei queue prefix ─> run-<pid>.scope in queue-<agent>.slice, LD_PRELOAD devgate.so
                                                     │ first device open: key=value lines
seimei serve ┬ routes, plugins, phone (main runtime)  │
             └ scheduler thread (own runtime) <───────┘  scans /proc + cgroupfs every 1 s, freezes, persists state
seimei bench, seimei queue <verb> ── JSON lines ──> the same socket
seimei queue keeper (only while something is frozen, outside seimei's cgroup) <── heartbeat ── scheduler
```

**One process: the scheduler runs inside `seimei serve`** (decision 1). It is a plain Rust module tree in a new crate `crates/queue` (`seimei-queue`): scheduler, scanner, cgroup and freeze code, the socket. serve starts it on its own OS thread with its own single-threaded tokio runtime; the rest of seimei talks to it over channels and never links its internals. `plugins/queue` keeps the tools and depends on the crate for the wire types.

Being inside serve means the queue now lives through serve's restarts: every deploy (several a day), `seimei update`, a crash and its respawn. The four rules that make that safe follow.

### Restarts keep the queue

A restart must not thaw a running bench, drop a lease or lose a waiter's place.

- **Persist.** The scheduler writes its live state to `$XDG_RUNTIME_DIR/seimei/queue/state.json` (tmpfs: it survives restarts, not reboots, which is right) by write-and-rename, coalesced to at most every 250 ms and at once before any freeze: leases (job id, cgroup, holder `pid:starttime`, resource, token), waiters (`req`, class, rank, position, since), grants, holds, bumps and parks, the bench (owner `pid:starttime`, token, start, settle), and every frozen slice and paused scope with why. Learned needs, history and policy stay in the state folder (`~/.local/share/seimei/queue/`), as cqd keeps them.
- **Re-adopt on start**, the path cqd already takes after a crash: read state.json; adopt holders by the fd scan and match them to their leases; a bench whose owner is still alive stays the bench, and `seimei bench` reconnects with its token; frozen slices stay frozen, taken back from the keeper; each waiter's place is kept for the grace window and given back to the shim that asks again with the same `req`; what nobody claims within the window is dropped.
- **The gap.** On SIGTERM serve writes `queue/restarting` with a deadline; the start removes it. devgate v3 (shipped with this) sees the marker on EOF and keeps reconnecting with the same `req` until the deadline instead of its 2 s, so a deploy costs a waiter nothing. Shims already mapped into running processes keep their 2 s and fall back to the v1 gate for the gap, which serializes safely: v1 scans for holders itself and waits on `bench.lock`, which the keeper holds through a bench (below). How long a deploy's gap is gets measured (V5).
- **Thaw only if seimei does not come back:** that is the keeper's job.

### The keeper (the watchdog that survives seimei)

The pick is a tiny helper, `seimei queue keeper` (same binary, a blocking loop, no runtime), not the supervisor's stop hook with a delay. Why:

- a stop hook runs inside the restart: `rc-service restart` and systemd both wait for it, so a delay long enough to cover the restart delays every deploy by that much;
- it does not run when serve crashes and is respawned, nor when serve is SIGKILLed, nor when the scheduler wedges while serve lives;
- whatever it backgrounds sits in the service's cgroup, which the stop kills (`cgroup.kill` in our OpenRC `stop_post`, `KillMode=control-group` under systemd);
- it is two scripts, one per init system; the keeper is the same code everywhere.

How it works:

1. The scheduler spawns the keeper the first time it freezes anything (a bench, a paused GPU job) and the keeper exits once nothing is frozen and the scheduler says so. With nothing frozen there is no process.
2. It runs outside serve's cgroup and outside every queue.slice, so neither a stop nor a bench takes it: under systemd a transient unit (`systemd-run --user --unit=seimei-queue-keeper --collect`), without systemd a leaf of its own at the top of the user's cgroup (`/<session>/seimei-keeper`, the same move `stop_post` makes into `seimei-ctl`; this user owns `/1` here).
3. It listens on `queue/keeper.sock`. The scheduler sends it a heartbeat every second with the frozen list (it also reads state.json), and at a bench's start hands it a dup of the locked `bench.lock` fd over SCM_RIGHTS. A flock belongs to the open file, so the lock holds while either process lives, and v1 shims keep waiting during a restart.
4. On EOF, or 10 s without a heartbeat (a wedged scheduler counts as gone), it starts the grace window, `[queue] restart_grace_s = 60`. A new serve connects, says `adopt`, gets the lock fd back, and the keeper goes on standing by. If the window runs out, it thaws exactly the recorded slices and scopes, closes the lock, writes an event to `queue/events.jsonl` ("seimei was not back in 60 s: thawed 12 slices"), and exits.
5. It also enforces `max_freeze_s` (3600) on its own clock, so a freeze ends even if every other piece fails.

The service's stop hook stops thawing (cqd's `--thaw-all` in `stop_post` is the rule this replaces). `seimei queue thaw-all` stays as the hand tool.

### The scheduler is isolated from serve's panics

- Its own OS thread and runtime: a route or plugin that panics or blocks the main runtime cannot stall a tick. The release profile already unwinds (`panic = "unwind"`), so a panicking request handler ends only itself.
- Each tick runs in `catch_unwind`. A panic is logged with its backtrace and an event, and the scheduler rebuilds itself from state.json plus a fresh scan, the same code as a start, without dropping its socket's connections (they live on the scheduler's runtime, outside the tick).
- Three panics in 60 s put it in safe mode: thaw everything, answer every `acquire` with `err` (shims take the v1 gate), post an urgent note, and stay so until serve restarts. The dispatcher's `queue` tool says it.
- The heartbeat is an atomic the tick bumps. serve watches it too: 10 s stale and it posts an urgent note ("the queue is stuck") with a card to restart. Nothing restarts serve on its own for this, because a restart ends seimei's sessions.

### Placement of serve itself

"Never restart from a queue slice" now applies to `seimei serve`: a bench freezes queue slices, and a serve inside one would freeze with it, its scheduler too. serve checks `/proc/self/cgroup` at start: inside a `queue.slice` it moves itself to the user's top cgroup leaf `seimei` (as the OpenRC `start_pre` does today) or, when it cannot, refuses to start with the reason. Setup's service templates (OpenRC and systemd) carry the same guard and the env clean-up of cqd's `start_pre` (no `LD_PRELOAD`, `CQ_*`, `CLAUDE*` from the shell that ran the restart).

### The queue without the phone server

`[server] web = false` runs serve with the scheduler, the queue socket and the plugins socket, and no HTTP port, tunnel or push: a workstation with three agents and one GPU. It is cheap because the web server is already one task among serve's. One scheduler per user: serve takes `~/.local/share/seimei/queue/lock` (on disk, since a new login session may get a new runtime dir while the last one's services live on), and the scheduler itself takes cqd's `cqd.lock` in its state dir, where a holder from an older login session is stopped and replaced; a second serve (a dev instance, a test) runs without the scheduler and says so in its log and in `queue` status.

**The protocol stays.** cqd's two line protocols are kept byte for byte: the shim's `key=value` lines (`acquire`, `hello`, `wait`, `ka`, `grant`, `pass`) and the JSON ops (`status`, `watch`, `bench`, `history`, `events`, `policy`, `control`, `ping`). devgate.so v2, every running session's mapped shim, `bench`, `cq` and seimei's plugin keep working against the new scheduler unchanged. Trust stays as cqd has it: SO_PEERCRED pid to cgroup; a process in an agent's queue slice reads and controls only its own leases, the rest may run every op.

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

- **Devices** (gpu, npu, and any path glob). devgate already takes its globs from `CLAUDE_QUEUE_DEVICES`; the scheduler hands each command the globs of every device resource, and a grant names the resource. `/dev/kfd` and `/dev/accel/*` are leased unless the desktop rules say otherwise; renderD alone is never "graphics" (Vulkan compute opens only renderD). Defaults: gpu and npu as above, coupled, as today. Several GPUs: one resource per card (`/dev/dri/renderD128` and `renderD129`, `nvidia0` and `nvidia1`), each its own turn.
- **Memory admission**, as cqd does it: need = p90 of a job key's learned peaks (anon + shmem + GTT/KFD buffers + half the file cache, x 1.15), else the declared `--mem`, else `mem_default_gb`; admit while need fits MemAvailable minus the reserve minus what running leases may still grow to; never force-grant past the reserve. On UMA (APUs) GPU memory is RAM, so this one gate covers both. Discrete VRAM: a later per-device gate from fdinfo `drm-resident-vram` against the card's total; out of scope for the first version.
- **CPU-heavy work** is shared, not queued: queue.slice gets a lower `cpu.weight` than the user's session (and benches freeze it), so a `cargo build -j32` in an agent never stutters the desktop and needs no wait. A declared `seimei queue run --take cpu -- ...` can still take an exclusive `cpu` token for jobs that must not overlap (two release builds fighting for LTO). The `nice -n 19` habit becomes unnecessary for queued agents.
- **User-defined tokens** (`slots = N`): taken explicitly with `--take`, for things devgate cannot see (an emulator, a licence seat, a rate-limited API). Matching processes by name is left out on purpose: exec interception is invasive and a wrong match blocks real work.

The `cpu` default and the token are decision 3; V4 checks the weight.

Policy (bench window, preemption, exempt agents, interactive daemons, desktop allowlist, memory numbers, event thresholds) keeps cqd's keys and defaults. Its file moves from `~/.config/claude-queue/policy.json` to seimei's state folder, `queue/policy.json`, because `queue_policy` writes it; config.toml holds only what the user sets by hand (resources, the cgroup root, a policy override).

The defaults stop naming this laptop: the exempt list becomes "seimei's dispatcher" by role (the scheduler knows seimei's own slice name), the desktop allowlist keeps the generic entries (browsers, electron gpu-process, Xwayland, terminals) and loses qs.py, z13gui, niri, waybar and friends, which move to this laptop's policy.json (done in the port; cqd and the shadow read the same policy.json, so the shadow agrees with cqd once they are there).

## Placement

The hierarchy stays: `<root>/queue.slice/queue-<agent>.slice/run-<pid>.scope`, desktop apps rehomed to `<root>/app.slice/app-cq-<name>.scope`.

**One root, decided by the scheduler.** The prefix never derives the root from its own cgroup again. The scheduler picks it at start and writes it to `$XDG_RUNTIME_DIR/seimei/queue/root`; the prefix reads that file. Under systemd the root is the user manager's (`user@<uid>.service`) and scopes are made with `systemd-run --user --scope --slice=...` as today (writing into a systemd-managed tree by hand would fight systemd). Without systemd it is elogind's delegated session cgroup (`/<session>/delegated`), and the prefix makes the scope itself, as queue-prefix does. This fixes problem 1.

**seimei's own sessions** (dispatcher, shikigami on Claude Code) already get the prefix through the plugin's `env` hook; nothing changes except the prefix being `seimei queue prefix` and the name coming from seimei. ACP and Codex shikigami have no shell prefix hook: seimei starts the agent process itself inside `queue-<name>.slice/agent.scope` with devgate preloaded, so the whole agent is the unit (its commands inherit both). A bench then freezes the agent's own process too; that is the price of an agent without a prefix hook, and it only pauses its API calls.

**The user's own Claude Code sessions, without the fish function.** Claude Code reads `env` from `~/.claude/settings.json` for every session, however it was started (a terminal, `--bg`, an IDE). `seimei setup` offers to add one entry:

```json
{"env": {"CLAUDE_CODE_SHELL_PREFIX": "/home/you/.local/bin/seimei queue prefix"}}
```

With no `CLAUDE_QUEUE` set, the prefix names the agent after the claude process that runs it: `q<pid>`, where the pid is its parent's, and `~/.claude/sessions/<pid>.json` is that session in Claude Code's registry. So seimei shows the session's name and folder for every queued command, which today needs a scan of `/proc/*/environ`. A session seimei starts keeps the name seimei gives it (`CLAUDE_QUEUE` in its environment wins). This is the user's Claude Code config (decision 2): setup shows the change as a unified diff of the whole file, asks, writes it by rename with the old file kept as `settings.json.bak`, and `seimei setup --undo-queue-env` takes the entry out again. The guard keeps agents from changing it. To verify before building (V1): that a settings `env` entry reaches `CLAUDE_CODE_SHELL_PREFIX` (it is read from the process environment; settings `env` is applied at startup in current versions, but this needs a test on the installed version), and how a value with a space is split (else a one-line wrapper script).

**Other agents in a terminal** (Codex, aider, gemini): `seimei queue run --agent NAME -- codex` places the whole agent, as for ACP shikigami. A shell alias is the user's choice; seimei does not install shell functions.

**serve and the keeper** are never in queue.slice (see Placement of serve itself, and the keeper's own cgroup).

## Benches and freezing

Kept exactly, because each rule is a measured fact (claude-queue's DESIGN-scheduler.md section 5, cqd's V4 notes):

- freeze through `cgroup.freeze`, never `systemctl freeze`; a scope created under a frozen cgroup starts frozen; keep the slice loaded while frozen; a poller freezes agents that appear mid-bench;
- record a slice before freezing it; thaw on bench end and after `max_freeze_s` (3600); a restart thaws nothing, and the keeper thaws when serve is not back within the grace window (see The keeper);
- a GPU holder is paused only after its GPU work drains (KFD cu_occupancy 0, SDMA and engine time flat for 1.5 s), else it is thawed at once and never paused again; never pause NPU users (unpinned buffers), unobservable jobs, exempt agents or benches;
- never `memory.reclaim` a cgroup that holds GPU/NPU fds.

`seimei bench [--settle ...] -- cmd` is today's `bench`: wait for its turn (it jumps the batch queue), freeze, settle CPU and temperature, run with its lease token, thaw, report contamination. With the one-root fix it freezes every agent, seimei's shikigami included.

## Without cgroup delegation

Some machines give the user no writable cgroup (no systemd user manager, elogind without `DelegateSessionCgroups=yes`, most containers). Then:

| feature | systemd user session | elogind, delegated | no delegation |
|---|---|---|---|
| GPU/NPU turns (devgate + scheduler) | yes | yes | yes: holders are the command's process tree (`CQ_LEADER` and its descendants, pidfd against reuse) instead of its scope |
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
| `queue` | cqd status, else a /proc snapshot | the same; status gains the mode (table above), safe mode, resources by name, and the user's sessions named from the registry |
| `queue_control` | bump, hold, unhold, pause, resume, revoke (kill behind a card), park, unpark, admit, grants | the same actions; targets by resource too (`gpu`, `emulator`) |
| `queue_policy` | get or set cqd's policy | the same keys, plus resources read-only (they are config) |
| `queue_history` | past jobs | the same, with the resource |
| triggers `queue_free`, `queue_wait` | yes | the same, per resource optionally (`queue_wait` target `gpu:10`) |
| worker note `{launch}` | systemd-run or niri spawn | `seimei queue launch -- <app>`: starts a desktop app outside the queue on any init and compositor, so the note no longer depends on the machine |

The plugin's /proc fallback stays for when the scheduler is off (a second serve, safe mode). The plugin's `wrap` (a session opened in a terminal stays queued) becomes the env entry instead of a `claude-queue` argv prefix.

## Migration on this laptop, with no downtime

Constraints: running Claude Code processes keep the environment they started with (`CLAUDE_CODE_SHELL_PREFIX=~/.local/lib/claude-queue/queue-prefix`, `CLAUDE_QUEUE_LIB=.../devgate.so`), the shim is mapped into running GPU jobs, old shims reconnect within 2 s or fall back to v1, and nothing may restart seimei or cqd from a queue slice.

0. **Done 2026-10-26 (claude-queue 39cee51):** cqd's bench, frozen list, leftover thaw, scope cleanup and hog scan cover every `<session>/*/delegated/queue.slice`. Verified live: a bench from the login session froze 16 agents in both roots, seimei's queue-dev included, and thawed them all.
1. **Done 2026-10-26: port with the old tests as the judge.** The Python tests start `cqd.py --runtime-dir --state-dir --config-dir ...` and talk to its socket. A test entry point, `seimei queue scheduler` with the same flags, runs the crate alone (no web, no plugins), so the same suite runs against both; the port is done when it passes unchanged. New Rust tests cover resources, the one-root rule, no-delegation mode, and the restart rules (below).
2. **Shadow** (running from 2026-10-26). `[queue] mode = "shadow"` in config.toml, and the default while cqd answers, picked up at the next deploy: the scheduler scans and classifies, binds no socket, freezes nothing, and logs where its view (holders, waits, desktop apps, frozen agents) differs from cqd's state.json for 15 ticks in a row, to `~/.local/share/seimei/queue-shadow/shadow-diff.jsonl` (cqd's own shadow.jsonl did this for v1 to v2). Deploys in this phase are ordinary. Finding, 2026-10-27: the first 6 rows (q4806 npu x3, phonewm_mockups gpu x3, 13 to 34 s each) were lease jobs that open the device in bursts. cqd held the lease the whole time; the shadow, with no socket, sees a holder only while an fd is open (reproduced: a job opening the render node 0.2 s in every 0.7 s shows as held, gone, held in the shadow). A shadow artifact, not a gap in the scheduler: from then on a cqd lease none of whose processes has a device open is not compared.
3. **Keep the old paths alive.** serve keeps writing `~/.local/lib/claude-queue/devgate.so` and `queue-prefix` by rename-into-place, so every running session's baked environment stays valid; queue-prefix becomes a two-line `exec seimei queue prefix "$1"`.
4. **Cut over at a deploy.** A script, started from the login session (never a queue slice), waits until no bench runs and nothing is frozen, stops cqd (nothing to thaw), sets `[queue] mode = "on"`, and restarts seimei through deploy.sh. The scheduler binds cqd's socket path, `$XDG_RUNTIME_DIR/claude-queue/cqd.sock`, adopts holders by fd scan and waiter positions from cqd's state.json (read once, the format both understand). Holders keep their fds throughout; waiters reconnect with the same `req` and keep their places, or fall back to v1 for the gap, which serializes safely. Then cqd's service leaves the default runlevel.
5. **Rollback** is the same step reversed: `mode = "off"`, a deploy, start cqd. The state file stays readable by both until cqd is retired.
6. **Placement next.** After V1 passes, setup adds the settings `env` entry; new sessions get `seimei queue prefix` and the one root. The fish function goes once no shell still has the old function loaded (open terminals keep it until they close; harmless, both prefixes land in the same root after step 0).
7. **Paths last.** Move to `$XDG_RUNTIME_DIR/seimei/queue.sock` and `~/.local/lib/seimei/devgate.so` with the old paths as symlinks until no process maps the old shim (`grep -l claude-queue/devgate.so /proc/*/maps`). Then archive the claude-queue repo with a pointer here.

## Distribution

- **devgate.so ships prebuilt** (decision 4). release.sh builds it for x86_64 glibc, against glibc 2.34 or later (`dlsym` moved into libc there; it needs only `dlsym`, `open`, `openat` and socket calls), and the release binary embeds it with `include_bytes!`, so the binary's minisign signature covers the shim too. serve (and setup) write it to `~/.local/lib/seimei/devgate.so` by rename-into-place when it differs. A source build compiles `crates/queue/devgate.c` in build.rs with the system C compiler instead, which also covers musl distributions.
- Requirements in docs/install.md: cgroup v2, and for the full feature set a systemd user session or elogind with delegated session cgroups. Linux only.

## Decisions (the user's answers, 2026-10-26)

1. **The queue runs inside `seimei serve`**, not as its own service. Restarts (deploys, `seimei update`) must not thaw a running bench or drop leases or waiters' places: the state is persisted and re-adopted on start, and a watchdog that survives seimei's death thaws only if seimei is not back within a grace window (the keeper, picked over a delayed stop hook, reasons in The keeper). The scheduler is isolated from route and plugin panics (own thread, `catch_unwind`, restart of the scheduler alone). "Never restart from a queue slice" now applies to serve. The queue without the phone server is `[server] web = false`.
2. **setup writes the env entry into `~/.claude/settings.json`**, after asking, with a clear diff shown.
3. **CPU:** a lower `cpu.weight` for agent slices by default, plus an opt-in exclusive `cpu` token.
4. **A prebuilt glibc devgate.so ships with releases.**
5. **One queue per user;** multi-user is out of scope.

## To verify before building

- V1: settings `env` reaches `CLAUDE_CODE_SHELL_PREFIX` in the installed Claude Code, and how it splits a value with a space.
- V2: the shim built against glibc 2.34 loads in the GPU programs used here (llama.cpp, torch, ollama, realesrgan) and in Vesktop and Firefox when they inherit it.
- V3: per-process-tree memory sampling (no delegation) tracks a ROCm job's peaks within 10% of memory.stat.
- V4: two agents' `cargo build -j32` with queue.slice at `cpu.weight` 20 keep niri frame times flat.
- V5: how long a deploy's gap is (SIGTERM, `stop_post`, start, socket bound), which says whether old shims keep their places or take v1 for it.
- V6: the keeper outlives `stop_post`'s `cgroup.kill`, a SIGKILLed serve and supervise-daemon's respawn, under OpenRC here and under systemd in a VM.

## Build steps

Each step ends green on its own tests and is committed and pushed; nothing reaches the live queue before step B10.

| step | what | estimate |
|---|---|---|
| B0 | the Python suite runs on OpenRC: a no-systemd path for its `systemd-run` scopes, so cqd_test, lease_test, preempt_test and bench_test run here against cqd first | 1.5 h |
| B1 | `crates/queue`: /proc and cgroupfs scanner, classification, policy file, state files, `seimei queue scheduler` entry point | 2.5 h |
| B2 | leases, memory admission and learned needs, ETAs, both protocols on the socket | 3 h |
| B3 | benches, freezing, GPU preemption with the drain checks, desktop rehoming | 3 h |
| B4 | inside serve: own thread and runtime, `catch_unwind` and safe mode, persistence and re-adoption, the restarting marker, the keeper, the cgroup self-check, `web = false`, the one-scheduler lock | 4 h |
| B5 | conformance: the Python suite against both; restart tests (restart mid-bench within the window: nothing thaws, waiters keep places; serve killed: the keeper thaws after the window; three panics: safe mode); V5, V6 | 3 h |
| B6 | placement: `seimei queue prefix`, the root file, devgate v3's reconnect, setup's settings.json diff; V1 | 2 h |
| B7 | resources and tokens, `cpu.weight`, the `cpu` token; V4 | 1.5 h |
| B8 | no-delegation mode; V3 | 2 h |
| B9 | the plugin tools, `seimei bench`, `seimei queue <verb>`, `queue launch` | 1.5 h |
| B10 | release.sh's glibc devgate (V2), docs, then shadow, cutover and the paths (Migration 2 to 7) | 1.5 h, plus 2 to 3 days of shadow |

About 25 hours of work, three to four days with reviews between the big steps (after B3, B5 and B6), then the shadow days before the cutover.
