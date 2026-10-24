"""Writes a realistic board (the shape of api/tasks) for the Tasks tab mockups: 8 projects, 23 open or paused items,
owners and states like the real board on 2026-10-24: one question, one job to review, three of the user's own to-dos,
two tasks running with steps, the rest queued, a few done per project. Usage: tasks_fixture.py OUT.json"""
import json, sys, time

now = time.time()
P = [("seimei", False), ("gentoo", False), ("home", False), ("efi-agent", False), ("personal", False), ("player", False),
     ("minecraft harness", True), ("yah cleanup", True)]

def steps(done, running, titles):
    out = []
    for i, t in enumerate(titles):
        out.append({"id": str(i + 1), "title": t, "state": "completed" if i < done else "in_progress" if i == done and running else "pending"})
    return out

T = [
    # seimei
    ("#55", "Task board", "seimei", "dev", "open", None, None),
    ("#60", "Tasks tab redesign mockups", "seimei", "m3x-review", "running", "a17c02de", steps(2, True, ["Read the board data", "Build a fixture", "Mock two directions", "Write it up"])),
    ("#57", "Queue order", "seimei", "dev", "open", None, None),
    ("#59", "After the queue", "seimei", "dev", "open", None, None),
    ("", "Back up the signing key", "seimei", "you", "open", None, None),
    # gentoo
    ("#61", "Kernel 7.3-rc5 rebuild", "gentoo", "gentoo", "running", "9b1e4471", steps(3, True, ["Sync the tree", "Apply the perfopt patches", "Build", "Install and update the EFI entry", "Reboot check"])),
    ("#62", "Pick a profile for the laptop", "gentoo", "gentoo", "asks", "9b1e4471", None),
    ("#63", "Drop systemd-boot leftovers", "gentoo", "gentoo", "open", None, None),
    ("#64", "Mesa 26.2 bump", "gentoo", "gentoo", "open", None, None),
    # home
    ("#65", "Tidy ~/Downloads", "home", "home-tidy", "review", "5d2a9e03", None),
    ("#66", "Sort the photo dump", "home", "home-tidy", "open", None, None),
    ("", "Renew the domain", "home", "you", "open", None, None),
    # efi-agent
    ("#67", "Boot entry for the rescue image", "efi-agent", "efi-agent", "open", None, None),
    ("#68", "Enroll Secure Boot keys", "efi-agent", "efi-agent", "open", None, None),
    # personal
    ("", "Call the dentist", "personal", "you", "open", None, None),
    # player
    ("#69", "Waveform seek bar", "player", "player", "open", None, None),
    ("#70", "Gapless playback", "player", "dev", "open", None, None),
    ("#71", "Scrobble on resume", "player", "dev", "open", None, None),
    # paused projects
    ("#40", "Headless client harness", "minecraft harness", "dev", "paused", None, None),
    ("#41", "World seed fixtures", "minecraft harness", "dev", "paused", None, None),
    ("#42", "Bot pathing test", "minecraft harness", "dev", "paused", None, None),
    ("#44", "Drop the old bench scripts", "yah cleanup", "dev", "paused", None, None),
    ("#45", "Merge the two queue configs", "yah cleanup", "dev", "paused", None, None),
]
DONE = [("#58", "Guard: grep patterns are not paths", "seimei", "seimei"), ("#47", "Metrics view", "seimei", "dev"), ("#48", "re: anchor marker", "seimei", "dev"), ("#52", "Remove the Screen tab", "seimei", "dev"),
        ("#53", "OpenRC user service", "seimei", "dev"), ("#54", "M3X review", "seimei", "m3x-review"),
        ("#50", "Rebuild world after the gcc bump", "gentoo", "gentoo"), ("#51", "Clean distfiles", "gentoo", "gentoo"),
        ("#43", "Sort the screenshots folder", "home", "home-tidy"), ("", "Fix the hallway lamp", "home", "you"),
        ("#46", "Dump the current boot entries", "efi-agent", "efi-agent"),
        ("#38", "Album art cache", "player", "dev"), ("#39", "Media keys", "player", "dev")]

tasks = []
for pos, (tid, title, proj, owner, state, worker, st) in enumerate(T):
    t = {"id": tid or f"u{pos}", "title": title, "project": proj, "owner": owner, "state": state, "done_at": None, "pos": pos,
         "worker": worker, "created": now - 86400, "updated": now - 600}
    if st:
        t["steps"] = st
    tasks.append(t)
for i, (tid, title, proj, owner) in enumerate(DONE):
    tasks.append({"id": tid or f"d{i}", "title": title, "project": proj, "owner": owner, "state": "done", "done_at": now - 3600 * (i + 1),
                  "pos": 100 + i, "worker": None, "created": now - 2 * 86400, "updated": now - 3600 * (i + 1)})
board = {"tasks": tasks, "projects": [{"name": n, "color": None, "paused": p} for n, p in P]}
json.dump(board, open(sys.argv[1], "w"), indent=1)
live = [t for t in tasks if t["state"] != "done"]
print(len(live), "open or paused,", len(tasks) - len(live), "done,", len(P), "projects")
