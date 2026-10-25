"""CSS linear() easings and durations for the M3 Expressive spring tokens (mass 1, from 0 to 1)."""
import math
SPECS = {"expressive spatial fast": (0.6, 800), "expressive spatial default": (0.8, 380), "expressive spatial slow": (0.8, 200),
         "effects fast": (1.0, 3800), "effects default": (1.0, 1600), "effects slow": (1.0, 800),
         "standard spatial fast": (0.9, 1400), "standard spatial default": (0.9, 700), "standard spatial slow": (0.9, 300)}
def run(z, k, dt=1/2000):
    x, v, t, out = 0.0, 0.0, 0.0, []
    c = 2 * z * math.sqrt(k)
    settle = 0
    while t < 3:
        a = k * (1 - x) - c * v
        v += a * dt; x += v * dt; t += dt
        out.append((t, x))
        if abs(1 - x) > 0.002 or abs(v) > 0.05: settle = t
    return [p for p in out if p[0] <= settle + dt], settle
for name, (z, k) in SPECS.items():
    pts, T = run(z, k)
    n = 16 if z < 1 else 10
    samp = [pts[min(len(pts) - 1, round(i * (len(pts) - 1) / n))][1] for i in range(n + 1)]
    samp[0], samp[-1] = 0, 1
    peak = max(p[1] for p in pts)
    print(f"{name}: {round(T * 1000)}ms overshoot {round((peak - 1) * 100, 1)}%  linear({', '.join(f'{s:.3f}'.rstrip('0').rstrip('.') if s else '0' for s in samp)})")
