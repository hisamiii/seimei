# Expressive pass (t18)

The showcase is good M3; this pass makes it recognizably M3 Expressive at the moments that earn it and leaves the dense
screens alone (REVIEW.md, where not to be expressive: chat, finished rows, Up next and usage meters stay standard).
The showcase itself is untouched: `build.py` reads `../showcase/` and writes the pages here, which load `shapes.css`
and `expressive.css` after `showcase.css`. Before and after images: `../../img/expressive/01` to `08`, all of them in
`all.png`.

## The moments

| # | Moment | What changes | Values |
|---|---|---|---|
| 1 | Needs you | Its own shape set against the routine groups: 28 outer, 8 inner corners, 3 gap, 80 rows; the asker's library shape on the tile; header title large emphasized, row titles title medium emphasized | ShapeTokens CornerExtraLarge 28, CornerSmall 8; TypeScaleTokens |
| 2 | Approval card | Allow a filled medium button with its icon, round at rest, 12 corners and wider when pressed (Deny gives way); Deny tonal at the same height | ButtonMediumTokens: 56 tall, pressed CornerMedium 12, icon 24, icon to label 8, leading 24 |
| 3 | Nav bar | The active pill takes primary with an on-primary icon (7.3:1 on the bar, was 1.3:1), the active label emphasized, the first tab named seimei (the top lane stays Chat) | NavigationBarTokens: 64 tall, indicator 56 x 32 full corners, LabelMedium |
| 4 | Running tasks | The linear wavy indicator, travelling, then a 4 gap, the flat track and its stop dot; usage meters stay flat | LinearProgressIndicatorTokens and WavyProgressIndicatorDefaults: amplitude 3, wavelength 40, wave height 10, thickness 4, gap 4, stop 4 |
| 5 | Top app bar | Agents, Tasks and Usage open on the large flexible bar (120, display small emphasized) and collapse to the small bar (64, title large) on a container tone when scrolled | AppBarLargeFlexibleTokens, AppBarSmallTokens |
| 6 | Shikigami tiles | A library shape per shikigami is its identity, the tone its state; a working one morphs to the soft burst and turns; finished ones stay circles | MaterialShapes |
| 7 | All clear | When nothing needs the user, the hero's place holds a very sunny shape that settles in, "All clear" in headline small emphasized and since when | MaterialShapes.VerySunny |
| 8 | Press | Shape first (one corner step down), then a squish on the fast spatial spring: send 0.92, buttons 0.96 wide, chips to 8 corners, the nav pill widens to 64 | MotionScheme expressive fast spatial |

Emphasized type is the baseline role one weight step up at the same size and line height: 400 to 500 for display,
headline, title large and body, 500 to 700 for title medium and small and the labels (TypeScaleTokens, androidx-main,
checked 2026-10-24).

Identity shapes: kv-gate pentagon, gentoo clover4, m3x-review cookie9, kernel sunny, prefill-tuning cookie6,
seimei-web puffy diamond, dotfiles gem, home-tidy flower. The still frames pin each working tile at a different point
of its morph (`PHASE` in `build.py`), so one image shows the shape, the burst and the way between.

## Where it departs from the spec

- The nav pill is primary, not secondary-container: in this dark theme the container tones sit at 1.3:1 on the bar.
- Allow is the medium button (56), not the large one (96): a 96 button is taller than half the card.
- The shapes are rebuilt from the MaterialShapes vertices with circular corner arcs; the smoothing some shapes carry
  is left out, so their corners read a touch sharper than on Android.
- The wave colour is baked into a data URI for the dark theme (`#cba6f7`); a light theme needs its own.
- A working tile shows no glyph: the morph is the activity indicator and replaces the loading indicator.
- Pressed states are drawn with `.is-pressed`, which stands in for `:active` plus its state layer.

## Rebuilding

```
python3 shapes.py shapes.css shapes-preview.html
python3 build.py
```

The images were shot in headless Firefox at 412 x 915, 2x, with infinite animations paused at 700 ms; `pairs.py SHOTDIR`
lays out the before and after frames from those shots for the pair images.
