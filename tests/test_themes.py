"""#81: the theme token files are what contrib/themes.py builds, every side carries every token the page and the app
use, and Catppuccin Mocha with mauve is still the page's own default (style.css :root)."""
import importlib.util
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
THEMES = ROOT / "seimei/web/themes"
spec = importlib.util.spec_from_file_location("themes", ROOT / "contrib/themes.py")
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)


def test_files_are_built():
    built = gen.build()
    assert json.loads((THEMES / "index.json").read_text()) == list(built)
    for name, f in built.items():
        assert json.loads((THEMES / f"{name}.json").read_text()) == f, f"run python3 contrib/themes.py ({name}.json is stale)"


def test_every_side_has_every_token():
    css = (ROOT / "seimei/web/style.css").read_text()
    root = dict(re.findall(r"^\s*--([a-z0-9-]+):\s*(#[0-9a-f]{6});", css[css.index(":root {"):css.index("}", css.index(":root {"))], re.M))
    for name, f in gen.build().items():
        if f.get("dynamic"):
            continue
        for side in ("dark", "light"):
            assert set(f[side]) - {"label", "tints"} == set(root), (name, side)
            assert len(f[side]["tints"]) == 10
    mocha = gen.build()["catppuccin"]["dark"]
    assert {k: mocha[k] for k in root} == root, "Mocha + mauve must stay the page's default"


def test_accents_cover_both_sides():
    f = gen.build()["catppuccin"]
    assert set(f["accents"]["dark"]) == set(f["accents"]["light"]) and len(f["accents"]["dark"]) == 14
    assert f["accents"]["dark"]["mauve"]["primary"] == f["dark"]["primary"]
