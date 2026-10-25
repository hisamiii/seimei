"""Builds the theme token files: web/themes/<theme>.json, read by the web page (theme.js) and bundled into
the app (its assets). A theme is a small base palette per side (dark and light); every token the page and the app use
comes from it, so adding a theme is one more entry here. Material You has no fixed colors: the page and the app derive
the same tokens from an M3 scheme (theme.js, Theme.kt). Run: python3 contrib/themes.py

The web page builds Material You from a seed with material-color-utilities, vendored as web/mcu.js. To update
it: `npm pack @material/material-color-utilities` into an empty folder, unpack it, and bundle an entry file that
re-exports Hct, SchemeTonalSpot, argbFromHex and hexFromArgb with `npx esbuild entry.js --bundle --format=esm --minify
--legal-comments=none`; keep the two header lines and copy the package's LICENSE to mcu.LICENSE."""
import json
from pathlib import Path

OUT = Path(__file__).resolve().parents[1] / "web/themes"


def rgb(h):
    h = h.lstrip("#")
    return [int(h[i:i + 2], 16) for i in (0, 2, 4)]


def mix(a, b, t):
    """t of a, the rest b (as CSS color-mix in srgb)."""
    return "#" + "".join(f"{round(x * t + y * (1 - t)):02x}" for x, y in zip(rgb(a), rgb(b)))


def extras(t, dark):
    """The tones built on the theme's own: status containers, the snack's accent, the asking card. theme.js and
    Theme.kt use the same rules for Material You."""
    s = t["surface"]
    on = (lambda c: mix(c, "#ffffff", .46)) if dark else (lambda c: mix(c, "#000000", .5))
    box = (lambda c: mix(c, s, .18)) if dark else (lambda c: mix(c, s, .14))
    return {"warn-container": box(t["warn"]), "on-warn-container": on(t["warn"]),
            "ok-container": box(t["ok"]), "on-ok-container": on(t["ok"]),
            "error-container": box(t["error"]), "on-error-container": on(t["error"]),
            "on-warn": mix(t["warn"], "#000000", .22) if dark else "#ffffff"}


def accent(a, p, dark):
    """The tokens that follow the accent."""
    s = p["surface"]
    return {"primary": a, "on-primary": mix(a, p["lowest"], .18) if dark else "#ffffff",
            "primary-container": mix(a, s, .16), "on-primary-container": mix(a, "#ffffff", .38) if dark else mix(a, "#000000", .55),
            "inverse-primary": mix(a, "#000000", .55) if dark else mix(a, "#ffffff", .45), "tint": mix(a, s, .07 if dark else .06)}


def side(p, dark):
    t = {"label": p["label"], "surface": p["surface"], "surface-low": p["low"], "surface-lowest": p["lowest"],
         "container": p["c1"], "container-high": p["c2"], "container-highest": p["c3"],
         **accent(p["accent"], p, dark),
         "tonal": mix(p["second"], p["c2"], .06) if dark else mix(p["second"], p["c1"], .14),
         "on-tonal": mix(p["second"], "#ffffff", .4) if dark else mix(p["second"], p["text"], .35),
         "ok": p["ok"], "warn": p["warn"], "error": p["error"],
         "text": p["text"], "text-2": p["text2"], "text-3": p["text3"], "outline": p["outline"]}
    t.update(extras(t, dark))
    t.update(p.get("exact", {}))
    t["tints"] = p["tints"]
    return t


MOCHA_TINTS = ["#89b4fa", "#a6e3a1", "#94e2d5", "#cba6f7", "#f5c2e7", "#b4befe", "#f9e2af", "#74c7ec", "#eba0ac", "#fab387"]
LATTE_TINTS = ["#1e66f5", "#40a02b", "#179299", "#8839ef", "#ea76cb", "#7287fd", "#df8e1d", "#209fb5", "#e64553", "#fe640b"]
CAT_ACCENTS = {
    "dark": {"rosewater": "#f5e0dc", "flamingo": "#f2cdcd", "pink": "#f5c2e7", "mauve": "#cba6f7", "red": "#f38ba8",
             "maroon": "#eba0ac", "peach": "#fab387", "yellow": "#f9e2af", "green": "#a6e3a1", "teal": "#94e2d5",
             "sky": "#89dceb", "sapphire": "#74c7ec", "blue": "#89b4fa", "lavender": "#b4befe"},
    "light": {"rosewater": "#dc8a78", "flamingo": "#dd7878", "pink": "#ea76cb", "mauve": "#8839ef", "red": "#d20f39",
              "maroon": "#e64553", "peach": "#fe640b", "yellow": "#df8e1d", "green": "#40a02b", "teal": "#179299",
              "sky": "#04a5e5", "sapphire": "#209fb5", "blue": "#1e66f5", "lavender": "#7287fd"},
}
# Catppuccin Mocha with mauve stays exactly the colors seimei had before themes.
MOCHA_EXACT = {"on-primary": "#2d1b47", "primary-container": "#3d3552", "on-primary-container": "#ecdcff",
               "tonal": "#35384d", "on-tonal": "#dfe3ff", "warn-container": "#45393e", "on-warn-container": "#ffdcc4",
               "ok-container": "#364142", "on-ok-container": "#c8f3c3", "error-container": "#45293a",
               "on-error-container": "#ffd0dc", "on-warn": "#3b2313", "inverse-primary": "#7028d9", "tint": "#2a2633"}

THEMES = {
    "catppuccin": {
        "name": "Catppuccin",
        "dark": {"label": "Mocha", "surface": "#1e1e2e", "low": "#181825", "lowest": "#11111b", "c1": "#26263a",
                 "c2": "#313244", "c3": "#3a3b50", "text": "#cdd6f4", "text2": "#bac2de", "text3": "#a6adc8",
                 "outline": "#6c7086", "ok": "#a6e3a1", "warn": "#fab387", "error": "#f38ba8", "accent": "#cba6f7",
                 "second": "#b4befe", "exact": MOCHA_EXACT, "tints": MOCHA_TINTS},
        "light": {"label": "Latte", "surface": "#eff1f5", "low": "#e6e9ef", "lowest": "#dce0e8", "c1": "#e6e9ef",
                  "c2": "#dce0e8", "c3": "#ccd0da", "text": "#4c4f69", "text2": "#5c5f77", "text3": "#6c6f85",
                  "outline": "#9ca0b0", "ok": "#40a02b", "warn": "#fe640b", "error": "#d20f39", "accent": "#8839ef",
                  "second": "#7287fd", "tints": LATTE_TINTS},
    },
    "rose-pine": {
        "name": "Rosé Pine",
        "dark": {"label": "Main", "surface": "#191724", "low": "#16141f", "lowest": "#121019", "c1": "#1f1d2e",
                 "c2": "#26233a", "c3": "#2f2c45", "text": "#e0def4", "text2": "#c5c2dc", "text3": "#908caa",
                 "outline": "#6e6a86", "ok": "#9ccfd8", "warn": "#f6c177", "error": "#eb6f92", "accent": "#c4a7e7",
                 "second": "#ebbcba", "tints": ["#9ccfd8", "#c4a7e7", "#ebbcba", "#f6c177", "#eb6f92", "#31748f",
                                                 "#e0def4", "#908caa", "#c4a7e7", "#9ccfd8"]},
        "light": {"label": "Dawn", "surface": "#faf4ed", "low": "#f4ede8", "lowest": "#efe6df", "c1": "#fffaf3",
                  "c2": "#f2e9e1", "c3": "#e8dfd8", "text": "#575279", "text2": "#6b6589", "text3": "#797593",
                  "outline": "#9893a5", "ok": "#286983", "warn": "#ea9d34", "error": "#b4637a", "accent": "#907aa9",
                  "second": "#d7827e", "tints": ["#56949f", "#907aa9", "#d7827e", "#ea9d34", "#b4637a", "#286983",
                                                  "#575279", "#797593", "#907aa9", "#56949f"]},
    },
    "gruvbox": {
        "name": "Gruvbox",
        "dark": {"label": "Dark", "surface": "#282828", "low": "#1d2021", "lowest": "#141617", "c1": "#32302f",
                 "c2": "#3c3836", "c3": "#504945", "text": "#ebdbb2", "text2": "#d5c4a1", "text3": "#bdae93",
                 "outline": "#7c6f64", "ok": "#b8bb26", "warn": "#fe8019", "error": "#fb4934", "accent": "#fabd2f",
                 "second": "#83a598", "tints": ["#83a598", "#b8bb26", "#8ec07c", "#d3869b", "#fabd2f", "#fe8019",
                                                 "#fb4934", "#83a598", "#8ec07c", "#d3869b"]},
        "light": {"label": "Light", "surface": "#fbf1c7", "low": "#f2e5bc", "lowest": "#ebdbb2", "c1": "#f9f5d7",
                  "c2": "#f2e5bc", "c3": "#ebdbb2", "text": "#3c3836", "text2": "#504945", "text3": "#665c54",
                  "outline": "#a89984", "ok": "#79740e", "warn": "#af3a03", "error": "#9d0006", "accent": "#b57614",
                  "second": "#076678", "tints": ["#076678", "#79740e", "#427b58", "#8f3f71", "#b57614", "#af3a03",
                                                  "#9d0006", "#076678", "#427b58", "#8f3f71"]},
    },
    "nord": {
        "name": "Nord",
        "dark": {"label": "Polar Night", "surface": "#2e3440", "low": "#292e39", "lowest": "#242933", "c1": "#3b4252",
                 "c2": "#434c5e", "c3": "#4c566a", "text": "#eceff4", "text2": "#e5e9f0", "text3": "#d8dee9",
                 "outline": "#616e88", "ok": "#a3be8c", "warn": "#d08770", "error": "#bf616a", "accent": "#88c0d0",
                 "second": "#81a1c1", "tints": ["#88c0d0", "#a3be8c", "#8fbcbb", "#b48ead", "#ebcb8b", "#81a1c1",
                                                 "#d08770", "#5e81ac", "#bf616a", "#8fbcbb"]},
        "light": {"label": "Snow Storm", "surface": "#eceff4", "low": "#e5e9f0", "lowest": "#d8dee9", "c1": "#f3f5f8",
                  "c2": "#e5e9f0", "c3": "#d8dee9", "text": "#2e3440", "text2": "#3b4252", "text3": "#4c566a",
                  "outline": "#9aa5b8", "ok": "#5e8a4a", "warn": "#b3633f", "error": "#bf616a", "accent": "#5e81ac",
                  "second": "#8fbcbb", "tints": ["#5e81ac", "#5e8a4a", "#4c8a89", "#8c6a8a", "#b08a3e", "#4c6a8f",
                                                  "#b3633f", "#3b5a80", "#bf616a", "#4c8a89"]},
    },
}


def build() -> dict[str, dict]:
    out = {}
    for key, t in THEMES.items():
        f = {"name": t["name"], "dark": side(t["dark"], True), "light": side(t["light"], False)}
        if key == "catppuccin":
            f["accent"] = "mauve"
            f["accents"] = {mode: {n: (accent(c, t[mode], mode == "dark") if not (mode == "dark" and n == "mauve") else
                                       {k: f["dark"][k] for k in ("primary", "on-primary", "primary-container",
                                                                  "on-primary-container", "inverse-primary", "tint")})
                                   for n, c in CAT_ACCENTS[mode].items()} for mode in ("dark", "light")}
        out[key] = f
    # Material You: the tokens come from an M3 scheme; the file names it and lends the fixed tones M3 has no role for.
    out["material-you"] = {"name": "Material You", "dynamic": True, "seed": "#6750a4",
                           "dark": {"ok": "#9cd67d", "warn": "#ffb68a", "tints": MOCHA_TINTS},
                           "light": {"ok": "#386a20", "warn": "#8b5000", "tints": LATTE_TINTS}}
    return out


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    files = build()
    for key, f in files.items():
        (OUT / f"{key}.json").write_text(json.dumps(f, indent=1, ensure_ascii=False) + "\n")
    (OUT / "index.json").write_text(json.dumps(list(files)) + "\n")
    print("wrote", ", ".join(f"{k}.json" for k in files), "and index.json in", OUT)
