# Generates the Sidefleet mark: the page on the left, fibres fanning out to two machines.
# Run `python3 mark.py` in this folder to rewrite every SVG here.
import math, sys, json

def bez(p, t):
    u = 1 - t
    return tuple(u*u*u*p[0][i] + 3*u*u*t*p[1][i] + 3*u*t*t*p[2][i] + t*t*t*p[3][i] for i in range(2))

def strands(src, dst, n, spread, hues, twist=0.0, samples=40, cbend=0.55):
    # Centreline leaves the source flat, then bends into the machine.
    dx = dst[0] - src[0]
    p = [src, (src[0] + dx*cbend, src[1]), (dst[0] - dx*cbend, dst[1]), dst]
    pts = [bez(p, k/samples) for k in range(samples+1)]
    out = []
    for i in range(n):
        s = (i/(n-1) - 0.5) * 2
        path = []
        for k, (x, y) in enumerate(pts):
            t = k/samples
            a, b = pts[max(0, k-1)], pts[min(samples, k+1)]
            vx, vy = b[0]-a[0], b[1]-a[1]; L = math.hypot(vx, vy) or 1
            nx, ny = -vy/L, vx/L
            env = math.sin(math.pi*t) ** 0.85
            off = s * spread/2 * env * math.cos(math.pi * t * twist)
            path.append((x + nx*off, y + ny*off))
        d = "M" + " L".join(f"{x:.2f} {y:.2f}" for x, y in path)
        out.append((d, hues[i % len(hues)] if len(hues) >= n else hues[int(i/n*len(hues))]))
    return out

# Sidefleet palette: cool for one machine, warm for the other, both from the app's status colours.
COOL = ["#3fb8c9", "#5c98ff", "#6f7cff", "#8b5cf6", "#a368f5"]
WARM = ["#ff6fae", "#f7797d", "#f08a2e", "#e9b13c", "#13a05a"]

def mark(size=32, n=7, spread=5.2, twist=0.0, sw=0.5, r=3.3, dots="currentColor", anim=False, bg=None, pad=0):
    S = (6.0, 16.0); A = (26.0, 7.5); B = (26.0, 24.5)
    parts = []
    vb = f"{-pad} {-pad} {32+2*pad} {32+2*pad}"
    if bg: parts.append(f'<rect x="{-pad}" y="{-pad}" width="{32+2*pad}" height="{32+2*pad}" rx="{(32+2*pad)*0.225:.2f}" fill="{bg}"/>')
    g = []
    for (dst, hues) in ((A, COOL), (B, WARM)):
        for d, c in strands(S, dst, n, spread, hues, twist):
            g.append(f'<path d="{d}" stroke="{c}"/>')
    parts.append(f'<g fill="none" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round" opacity=".95">' + "".join(g) + "</g>")
    if anim:
        a = []
        for j, (dst, hues) in enumerate(((A, COOL), (B, WARM))):
            for i, (d, c) in enumerate(strands(S, dst, n, spread, hues, twist)):
                a.append(f'<path class="pulse p{j}" style="--i:{i}" d="{d}" stroke="{c}" pathLength="1"/>')
        parts.append(f'<g fill="none" stroke-width="{sw*1.5:.2f}" stroke-linecap="round">' + "".join(a) + "</g>")
    parts.append(f'<g fill="{dots}"><circle cx="{S[0]}" cy="{S[1]}" r="{r}"/><circle cx="{A[0]}" cy="{A[1]}" r="{r}"/><circle cx="{B[0]}" cy="{B[1]}" r="{r}"/></g>')
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}" width="{size}" height="{size}">' + "".join(parts) + "</svg>"

# The full mark, and a three-strand cut for 32px and below where hairlines vanish.
BIG = dict(n=9, spread=6.2, sw=0.42, r=2.7)
SMALL = dict(n=3, spread=4.4, sw=1.6, r=3.2)

# Dots follow the page: ink on light, paper on dark.
AUTO = '<style>circle{fill:#191919}@media(prefers-color-scheme:dark){circle{fill:#ececE8}}</style>'
# Pulses run from the page to the top machine, then the bottom one; each machine rings as one lands.
ANIM = (
    '<style>'
    '.pulse{stroke-dasharray:.11 1.4;stroke-dashoffset:.11;animation:run 3.2s cubic-bezier(.45,0,.25,1) infinite;animation-delay:calc(var(--i)*25ms)}'
    '.p1{animation-delay:calc(1.6s + var(--i)*25ms)}'
    '@keyframes run{0%{stroke-dashoffset:.11}50%,100%{stroke-dashoffset:-1}}'
    '.ring{fill:none;stroke-width:.6;opacity:0;transform-box:fill-box;transform-origin:center;animation:ring 3.2s ease-out infinite}'
    '.ring.b{animation-delay:1.6s}'
    '@keyframes ring{0%,44%{opacity:0;transform:scale(1)}48%{opacity:.7;transform:scale(1)}70%,100%{opacity:0;transform:scale(2.1)}}'
    '@media(prefers-reduced-motion:reduce){.pulse,.ring{animation:none;opacity:0}}'
    '</style>'
)

def write(name, svg):
    open(name, "w").write(svg + "\n")

if __name__ == "__main__":
    write("mark.svg", mark(size=64, dots="#191919", **BIG).replace("</svg>", AUTO + "</svg>"))
    write("mark-light.svg", mark(size=64, dots="#191919", **BIG))
    write("mark-dark.svg", mark(size=64, dots="#ececE8", **BIG))
    write("mark-small.svg", mark(size=32, dots="#191919", **SMALL).replace("</svg>", AUTO + "</svg>"))
    write("app-icon.svg", mark(size=512, dots="#ececE8", bg="#161615", pad=7, **BIG))
    write("app-icon-small.svg", mark(size=64, dots="#ececE8", bg="#161615", pad=5, **SMALL))
    # Android crops maskable icons to any shape inside the middle 80%: full-bleed ground, more room.
    write("maskable.svg", mark(size=512, dots="#ececE8", bg="#161615", pad=14, **BIG).replace('rx="13.50"', 'rx="0"'))
    anim = mark(size=128, dots="#191919", anim=True, **BIG).replace('opacity=".95"', 'opacity=".42"', 1)
    rings = '<g stroke="#8b5cf6"><circle class="ring a" cx="26" cy="7.5" r="2.7"/></g><g stroke="#f08a2e"><circle class="ring b" cx="26" cy="24.5" r="2.7"/></g>'
    write("mark-animated.svg", anim.replace("</svg>", rings + AUTO.replace("circle{", "circle:not(.ring){").replace("circle{", "circle:not(.ring){") + ANIM + "</svg>"))
