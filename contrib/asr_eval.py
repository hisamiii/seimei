#!/usr/bin/env python3
"""Dictation eval (docs/asr.md): play clips through dictate's direct OpenRouter path and through seimei's asr.sock,
and score jargon accuracy, WER, terms inserted on clean clips, fixer hits, latency and cost.

    asr_eval.py fixture FILE            save the live context (chat, names, board) as a fixture
    asr_eval.py run FIXTURE [ROUNDS]    both paths over the clips, ROUNDS times each (default 2)
    asr_eval.py replay                  the fixer over the phrases in misses.log, for checking by hand

Clip folders hold WAVs and a truth.json of {clip: text}. Set ASR_EVAL_CLIPS to a colon-separated list of them;
ASR_EVAL_DICTATE to dictate's checkout, ASR_EVAL_SOCK to another seimei's asr.sock. Results go to ASR_EVAL_OUT (default: the current folder)."""

import http.client
import json
import os
import re
import socket
import statistics
import sys
import time

HOME = os.path.expanduser("~")
SOCK = os.environ.get("ASR_EVAL_SOCK", os.path.join(HOME, ".local/share/seimei/asr.sock"))
CLIPS = os.environ.get("ASR_EVAL_CLIPS", ":".join(
    os.path.join(HOME, "Development/Scratch", d) for d in ("dictate/jargon", "dictate/clips", "asr-eval/clean")))
DICTATE = os.environ.get("ASR_EVAL_DICTATE", os.path.join(HOME, "Development/Projects/dictate"))
OUT = os.environ.get("ASR_EVAL_OUT", ".")
# The terms the jargon clips were written around.
JARGON = ["seimei", "niri", "kubus", "Gentoo", "shikigami", "Mod+H", "gfx1151", "loom", "hrx"]


class Unix(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=60)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.path)


class Seimei:
    def __init__(self):
        self.c = Unix(SOCK)

    def call(self, path, body, headers):
        for attempt in range(2):
            try:
                self.c.request("POST", path, body, headers)
                r = self.c.getresponse()
                data = r.read()
                break
            except (OSError, http.client.HTTPException):
                self.c.close()
                if attempt:
                    raise
        if r.status != 200:
            raise RuntimeError(f"HTTP {r.status}: {data[:300].decode(errors='replace')}")
        return json.loads(data)

    def context(self, hint, text=None):
        body = {"hint": hint} if text is None else {"hint": hint, "text": text}
        return self.call("/context", json.dumps(body), {"Content-Type": "application/json"})

    def transcribe(self, wav, hint):
        return self.call("/asr", wav, {"Content-Type": "audio/wav", "X-Seimei-Asr": json.dumps(hint)})


def dictate():
    sys.path.insert(0, DICTATE)
    import dictate as d
    import vocab
    return d, vocab


def clips():
    out = []
    for folder in CLIPS.split(":"):
        truth = json.load(open(os.path.join(folder, "truth.json")))
        kind = "jargon" if "jargon" in folder else "plain" if folder.endswith("clips") else "clean"
        out += [(kind, os.path.join(folder, name), text) for name, text in sorted(truth.items())]
    return out


def nw(s):
    return re.sub(r"[^\w+ ]", " ", re.sub(r"\s*\+\s*", "+", s.lower())).split()


def ed(a, b):
    d = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        p, d[0] = d[0], i
        for j, y in enumerate(b, 1):
            p, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, p + (x != y))
    return d[-1]


def has(term, text):
    t = re.sub(r"\s*\+\s*", "+", text.lower())
    return re.search(r"(?<![\w+])" + re.escape(term.lower()) + r"(?![\w+])", t) is not None


def score(rows, lists):
    """rows: (path, kind, clip, ref, text, ms, cost, fixed)."""
    out = {}
    for path in dict.fromkeys(r[0] for r in rows):
        rs = [r for r in rows if r[0] == path and r[4] is not None]
        errors = sum(1 for r in rows if r[0] == path and r[4] is None)
        we = sum(ed(nw(r[3]), nw(r[4])) for r in rs)
        wn = sum(len(nw(r[3])) for r in rs)
        want = got = 0
        for r in rs:
            if r[1] == "jargon":
                terms = [j for j in JARGON if has(j, r[3])]
                want += len(terms)
                got += sum(has(j, r[4]) for j in terms)
        inserted = sorted({t for r in rs if r[1] == "clean" for t in lists[path] if has(t, r[4]) and not has(t, r[3])})
        lat = sorted(r[5] for r in rs)
        out[path] = {
            "n": len(rs), "errors": errors,
            "jargon": f"{got}/{want}", "jargon_pct": round(100 * got / want, 1) if want else None,
            "wer_pct": round(100 * we / wn, 2) if wn else None,
            "wer_clean_pct": round(100 * sum(ed(nw(r[3]), nw(r[4])) for r in rs if r[1] != "jargon")
                                   / max(1, sum(len(nw(r[3])) for r in rs if r[1] != "jargon")), 2),
            "inserted": inserted,
            "fixed": sum(len(r[7]) for r in rs),
            "latency_median_ms": round(statistics.median(lat)) if lat else None,
            "latency_p90_ms": round(lat[int(len(lat) * 0.9)]) if lat else None,
            "cost": round(sum(r[6] or 0 for r in rs), 5),
        }
    return out


def run(fixture, rounds):
    d, vocab = dictate()
    api = d.Api(d.load_config())
    api.warm(2)
    pins, auto = vocab.pins_and_auto()
    direct_list = vocab.merge(pins, auto, cap=api.cfg["vocab_max"])
    fixes = vocab.load_fixes()
    s = Seimei()
    hint = {"fixture": json.load(open(fixture))["context"], "client": "eval"}
    lists = {"direct": direct_list, "seimei": [t for t, _ in s.context(hint)["terms"]]}
    cs = clips()
    rows = []

    def direct(wav):
        t = time.monotonic()
        text, model, cost = api.transcribe(wav, words=direct_list)
        ms = (time.monotonic() - t) * 1000
        return (None if text is None else vocab.apply_fixes(text, fixes)), ms, cost, []

    def seimei(wav):
        t = time.monotonic()
        try:
            r = s.transcribe(wav, hint)
        except Exception as e:
            print("seimei:", e, file=sys.stderr)
            return None, 0, None, []
        return r["text"], (time.monotonic() - t) * 1000, r.get("cost"), r.get("fixed") or []

    # One call each first, so neither pays for opening its connections.
    warm = open(cs[0][1], "rb").read()
    direct(warm), seimei(warm)
    for n in range(rounds):
        for i, (kind, clip, ref) in enumerate(cs):
            wav = open(clip, "rb").read()
            # Alternate which path goes first, so neither always gets the warmer connection.
            order = [("direct", direct), ("seimei", seimei)]
            if (i + n) % 2:
                order.reverse()
            for path, f in order:
                text, ms, cost, fixed = f(wav)
                rows.append((path, kind, os.path.basename(clip), ref, text, ms, cost, fixed))
            print(f"\r{n + 1}/{rounds} {i + 1}/{len(cs)}", end="", file=sys.stderr, flush=True)
    print(file=sys.stderr)
    result = {"t": time.strftime("%Y-%m-%d %H:%M"), "rounds": rounds, "scores": score(rows, lists),
              "lists": {k: len(v) for k, v in lists.items()},
              "diffs": [r for r in rows if r[4] is not None and nw(r[3]) != nw(r[4])]}
    path = os.path.join(OUT, f"asr-eval-{time.strftime('%Y%m%d-%H%M')}.json")
    json.dump(result, open(path, "w"), indent=1)
    print(json.dumps(result["scores"], indent=1))
    print("wrote", path)


def replay():
    """Each logged phrase through dictate's suspects() and seimei's fixer, with seimei's live list."""
    d, vocab = dictate()
    pins, auto = vocab.pins_and_auto()
    known = pins + auto
    s = Seimei()
    log = os.path.join(HOME, ".local/share/seimei/asr/misses.log")
    changed = resolved = flagged = 0
    for line in open(log, encoding="utf-8", errors="replace"):
        parts = line.rstrip("\n").split("\t")
        if len(parts) < 2 or parts[1].startswith("edited:"):
            continue
        text = parts[1]
        old = vocab.suspects(text, known)
        new = s.context({"chat": "seimei"}, text)["fixed"]
        flagged += len(old)
        if new["fixed"]:
            changed += 1
            resolved += len(new["fixed"])
            print(f"{text}\n  -> {new['text']}\n  fixed {new['fixed']}")
        print(f"  dictate flags {old}; seimei leaves {new['suspects']}")
    print(f"dictate flagged {flagged}; seimei fixed {resolved} in {changed} phrases (check each change above by hand)")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "fixture" and len(sys.argv) > 2:
        ctx = Seimei().context({"chat": "seimei"})
        json.dump(ctx, open(sys.argv[2], "w"), indent=1)
        print(f"{len(ctx['terms'])} terms; saved {sys.argv[2]}")
    elif cmd == "run" and len(sys.argv) > 2:
        run(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 2)
    elif cmd == "replay":
        replay()
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main()
