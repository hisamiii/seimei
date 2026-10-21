import pathlib
"""kitty.py screen parsing on captured Claude Code screens: drafts, dialogs, survey, question block."""
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from seimei import kitty  # noqa: E402
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, str(x)[:200])
R = "─" * 40
prompt = lambda line: f"\x1b[m\x1b[37m{R} Claude -p ─\n\x1b[m❯\xa0{line}\n\x1b[m\x1b[37m{R}\n\x1b[m  \x1b[37mHaiku 5.5\n\n\n"
check("faint placeholder is no draft", kitty.draft(prompt('\x1b[22;2mTry "fix lint errors"')) == "")
check("typed text is a draft", kitty.draft(prompt("half typed")) == "half typed")
check("empty prompt", kitty.draft(prompt("")) == "")
sv = "● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n" + prompt("")
check("survey seen", kitty.survey(sv) and not kitty.dialog(sv))
check("no survey on a plain prompt", not kitty.survey(prompt("")))
q = f""" ☐ Fruit \nWhich fruit do you want?\n❯ 1. Apple\n     Apple\n  2. Pear\n     Pear\n  3. Type something.\n{R}\n  4. Chat about this\nEnter to select · ↑/↓ to navigate · Esc to cancel\n\n\n\n{R} Claude -p ─\n\n\n"""
check("question pressable", kitty._visible(q, "Which fruit do you want?", 2, "Pear"))
check("wrong label refused", not kitty._visible(q, "Which fruit do you want?", 2, "Apple"))
check("other question refused", not kitty._visible(q, "Which drink?", 2, "Pear"))
stale = "Which fruit do you want?\n  2. Pear\n" + prompt("")
check("stale question in scrollback refused", not kitty._visible(stale, "Which fruit do you want?", 2, "Pear"))
perm = f"Do you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel\n" + prompt("")
check("permission prompt is no question", not kitty._visible(perm, "Do you want to proceed?", 1, "Yes") and kitty.dialog(perm))
ms = q.replace(" ☐ Fruit ", "←  ☐ Toppings  ✔ Submit  →")
check("multi-select / multi-question refused", not kitty._visible(ms, "Which fruit do you want?", 2, "Pear"))
here = pathlib.Path(__file__).parent / "screens"
check("bash permission prompt", kitty.permission((here / "perm-bash.txt").read_text()) == "Bash command: touch /tmp/permtest-sessev")
check("write permission prompt names the file", kitty.permission((here / "perm-write.txt").read_text()) == "Create file: perm-note.txt")
check("no permission prompt on a question", kitty.permission(q) is None and kitty.permission(prompt("")) is None)
print(f"{sum(res)}/{len(res)} passed")
