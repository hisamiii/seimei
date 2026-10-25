"""Writes shapes.css: shapes of the M3 Expressive library as clip-path polygons. Usage: shapes.py OUT.css [PREVIEW.html]

The vertices and corner roundings are the ones in androidx MaterialShapes.kt (androidx-main, checked 2026-10-24); the
corners are rounded the way graphics-shapes RoundedPolygon does it (cut = r * cot(angle / 2), scaled down when two cuts
don't fit on one side), with circular arcs: the smoothing some shapes carry is left out. Every shape is resampled at the
same N angles around its center, so clip-path can interpolate between any two of them (the working morph)."""
import math, sys

N = 120


def star(n, inner, r):
    # RoundedPolygon.star: outer vertices at 2*pi*i/n, inner ones halfway between, every corner rounded by r
    pts = []
    for i in range(n):
        a = 2 * math.pi * i / n
        pts.append(((math.cos(a), math.sin(a)), r))
        a = 2 * math.pi * (i + .5) / n
        pts.append(((inner * math.cos(a), inner * math.sin(a)), r))
    return pts


def rotate(pts, deg):
    c, s = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    return [((x * c - y * s, x * s + y * c), r) for (x, y), r in pts]


def custom(pnr, reps, mirroring=False, center=(.5, .5)):
    # MaterialShapes.doRepeat
    cx, cy = center
    out = []
    if mirroring:
        ang = [math.degrees(math.atan2(y - cy, x - cx)) for (x, y), _ in pnr]
        dist = [math.hypot(x - cx, y - cy) for (x, y), _ in pnr]
        sect = 360 / (reps * 2)
        for it in range(reps * 2):
            for index in range(len(pnr)):
                i = index if it % 2 == 0 else len(pnr) - 1 - index
                if i > 0 or it % 2 == 0:
                    a = math.radians(sect * it + (ang[i] if it % 2 == 0 else sect - ang[i] + 2 * ang[0]))
                    out.append(((math.cos(a) * dist[i] + cx, math.sin(a) * dist[i] + cy), pnr[i][1]))
    else:
        for k in range(len(pnr) * reps):
            (x, y), r = pnr[k % len(pnr)]
            a = math.radians((k // len(pnr)) * 360 / reps)
            dx, dy = x - cx, y - cy
            out.append(((dx * math.cos(a) - dy * math.sin(a) + cx, dx * math.sin(a) + dy * math.cos(a) + cy), r))
    return out


def outline(pts, steps=24):
    """The rounded outline as a dense polyline."""
    n = len(pts)
    V = [p for p, _ in pts]
    R = [r for _, r in pts]
    cut, info = [], []
    for i in range(n):
        p0, p1, p2 = V[i - 1], V[i], V[(i + 1) % n]
        d1 = (p0[0] - p1[0], p0[1] - p1[1]); l1 = math.hypot(*d1); d1 = (d1[0] / l1, d1[1] / l1)
        d2 = (p2[0] - p1[0], p2[1] - p1[1]); l2 = math.hypot(*d2); d2 = (d2[0] / l2, d2[1] / l2)
        cosA = d1[0] * d2[0] + d1[1] * d2[1]
        sinA = math.sqrt(max(0, 1 - cosA * cosA))
        cut.append(R[i] * (cosA + 1) / sinA if sinA > 1e-3 else 0)
        info.append((d1, d2, cosA, sinA))
    ratio = []
    for i in range(n):
        j = (i + 1) % n
        side = math.dist(V[i], V[j])
        need = cut[i] + cut[j]
        ratio.append(min(1, side / need) if need > 0 else 1)
    poly = []
    for i in range(n):
        p1 = V[i]
        d1, d2, cosA, sinA = info[i]
        c = cut[i] * min(ratio[i - 1], ratio[i])
        if c <= 0 or R[i] <= 0:
            poly.append(p1); continue
        r = R[i] * c / cut[i]
        a0 = (p1[0] + d1[0] * c, p1[1] + d1[1] * c)
        a1 = (p1[0] + d2[0] * c, p1[1] + d2[1] * c)
        bis = (d1[0] + d2[0], d1[1] + d2[1]); bl = math.hypot(*bis); bis = (bis[0] / bl, bis[1] / bl)
        cd = math.hypot(r, c)
        ctr = (p1[0] + bis[0] * cd, p1[1] + bis[1] * cd)
        t0 = math.atan2(a0[1] - ctr[1], a0[0] - ctr[0]); t1 = math.atan2(a1[1] - ctr[1], a1[0] - ctr[0])
        dt = (t1 - t0 + math.pi) % (2 * math.pi) - math.pi
        for k in range(steps + 1):
            t = t0 + dt * k / steps
            poly.append((ctr[0] + r * math.cos(t), ctr[1] + r * math.sin(t)))
    return poly


def normalized(poly):
    xs, ys = [p[0] for p in poly], [p[1] for p in poly]
    w, h = max(xs) - min(xs), max(ys) - min(ys)
    s = 1 / max(w, h)
    ox, oy = (1 - w * s) / 2 - min(xs) * s, (1 - h * s) / 2 - min(ys) * s
    return [(x * s + ox, y * s + oy) for x, y in poly]


def resample(poly):
    """Radius of the outline at N angles around the center (the farthest crossing)."""
    out = []
    for k in range(N):
        t = 2 * math.pi * (k + .25) / N - math.pi / 2
        dx, dy = math.cos(t), math.sin(t)
        best = 0
        for i in range(len(poly)):
            (x1, y1), (x2, y2) = poly[i], poly[(i + 1) % len(poly)]
            ex, ey = x2 - x1, y2 - y1
            den = dx * ey - dy * ex
            if abs(den) < 1e-12:
                continue
            qx, qy = x1 - .5, y1 - .5
            s = (qx * ey - qy * ex) / den
            u = (qx * dy - qy * dx) / den
            if s > 0 and -1e-9 <= u <= 1 + 1e-9:
                best = max(best, s)
        out.append((.5 + dx * best, .5 + dy * best))
    return out


SHAPES = {
    "circle": [((math.cos(2 * math.pi * i / 64) / 2 + .5, math.sin(2 * math.pi * i / 64) / 2 + .5), 0) for i in range(64)],
    "cookie4": custom([((1.237, 1.236), .258), ((.500, .918), .233)], 4),
    "cookie6": custom([((.723, .884), .394), ((.500, 1.099), .398)], 6),
    "cookie9": rotate(star(9, .8, .5), -90),
    "cookie12": rotate(star(12, .8, .5), -90),
    "clover4": custom([((.500, .074), 0), ((.725, -.099), .476)], 4, mirroring=True),
    "clover8": custom([((.500, .036), 0), ((.758, -.101), .209)], 8),
    "sunny": star(8, .8, .15),
    "very-sunny": custom([((.500, 1.080), .085), ((.358, .843), .085)], 8),
    "soft-burst": custom([((.193, .277), .053), ((.176, .055), .053)], 10),
    "flower": custom([((.370, .187), 0), ((.416, .049), .381), ((.479, .001), .095)], 8, mirroring=True),
    "pentagon": custom([((.500, -.009), .172), ((1.030, .365), .164), ((.828, .970), .169)], 1, mirroring=True),
    "gem": custom([((.499, 1.023), .241), ((-.005, .792), .208), ((.073, .258), .228), ((.433, -.000), .491)], 1, mirroring=True),
    "puffy-diamond": custom([((.870, .130), .146), ((.818, .357), 0), ((1.000, .332), .853)], 4, mirroring=True),
}


def css_polygon(pts):
    return "polygon(" + ", ".join(f"{x * 100:.2f}% {y * 100:.2f}%" for x, y in pts) + ")"


if __name__ == "__main__":
    lines = ["/* M3 Expressive shape library as clip-paths. Generated by shapes.py from androidx MaterialShapes.kt,",
             f"   {N} points each so any two interpolate. */", ":root {"]
    done = {}
    for name, pts in SHAPES.items():
        done[name] = resample(normalized(outline(pts)))
        lines.append(f"  --shape-{name}: {css_polygon(done[name])};")
    lines.append("}")
    for name in SHAPES:
        lines.append(f".shape-{name} {{ clip-path: var(--shape-{name}); }}")
    open(sys.argv[1], "w").write("\n".join(lines) + "\n")
    if len(sys.argv) > 2:
        cells = "".join(f'<figure><i class="shape-{n}"></i><figcaption>{n}</figcaption></figure>' for n in SHAPES)
        open(sys.argv[2], "w").write(f"""<!doctype html><meta charset="utf-8"><title>shape library</title>
<link rel="stylesheet" href="shapes.css"><style>
body {{ margin: 0; padding: 16px; background: #1e1e2e; color: #cdd6f4; font: 12px sans-serif; display: grid; grid-template-columns: repeat(7, 96px); gap: 12px; width: max-content; }}
figure {{ margin: 0; text-align: center; }} i {{ display: block; width: 80px; height: 80px; margin: 0 auto 6px; background: #cba6f7; }}
</style>{cells}""")
    print("wrote", len(SHAPES), "shapes")
