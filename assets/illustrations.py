import sys, os
OUT = sys.argv[1]
BENCH, SURF, TERM, INK, MUTED, LINE, SHELF = "#0E161C", "#162029", "#090F13", "#E7EEF2", "#96A6B1", "#2A3843", "#3A4B58"
TAPE, TAPEINK, FREE, NO, HINT = "#F4C534", "#13202B", "#4CC38A", "#FF9C86", "#8FC7F2"
DISP = "'Bricolage Grotesque','Arial Narrow',Arial,sans-serif"
BODY = "'Instrument Sans',system-ui,-apple-system,'Segoe UI',Arial,sans-serif"
MONO = "'JetBrains Mono',ui-monospace,Menlo,Consolas,monospace"

def t(x, y, s, size, fill=INK, font=BODY, weight=400, anchor="start", extra=""):
    return f'<text x="{x}" y="{y}" font-family="{font}" font-size="{size}" font-weight="{weight}" fill="{fill}" text-anchor="{anchor}" {extra}>{s}</text>'

SPARK = "#E08A6D"
SCALE = 1.0

def spark(cx, cy, r=11):
    """An eight-ray spark drawn for this project: the mark for an agent session."""
    import math
    rays = []
    for i in range(8):
        ang = math.radians(i * 45 + 12)
        k = 1.0 if i % 2 == 0 else 0.72
        rays.append(f'M{cx + math.cos(ang)*r*0.22:.1f} {cy + math.sin(ang)*r*0.22:.1f} L{cx + math.cos(ang)*r*k:.1f} {cy + math.sin(ang)*r*k:.1f}')
    return f'<path d="{" ".join(rays)}" stroke="{SPARK}" stroke-width="3.4" stroke-linecap="round" fill="none"/>'

def agent(x, y, w, h, name, cmd, sub="Claude Code"):
    cw = len(cmd) * 10.8
    return "".join([
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" fill="{TERM}" stroke="{LINE}" stroke-width="2"/>',
        spark(x + 34, y + 30),
        t(x + 56, y + 38, name, 24, TAPE, BODY, 600),
        t(x + w - 22, y + 37, sub, 15, MUTED, BODY, 400, "end"),
        t(x + 24, y + h - 26, "$", 18, MUTED, MONO), t(x + 46, y + h - 26, cmd, 18, INK, MONO),
        f'<rect x="{x + 52 + cw}" y="{y + h - 42}" width="9" height="20" fill="{MUTED}" opacity="0.7"/>',
    ])

def phone(cx, cy, held, tv=False, bad=False, emu=False):
    """A device drawing: body, screen with a small app layout, camera, side key."""
    edge = NO if bad else INK
    screen = TAPE if held else ("#2A1512" if bad else SURF)
    ui = TAPEINK if held else (NO if bad else SHELF)
    op = "0.55" if held else "1"
    g = [f'<g transform="translate({cx} {cy}) scale({SCALE})">']
    if tv:
        w, h = 104, 62
        g.append(f'<rect x="{-w/2}" y="{-h/2 - 6}" width="{w}" height="{h}" rx="6" fill="{TERM}" stroke="{edge}" stroke-width="3.5"/>')
        g.append(f'<rect x="{-w/2 + 6}" y="{-h/2}" width="{w - 12}" height="{h - 12}" rx="2" fill="{screen}"/>')
        for i in range(3):
            g.append(f'<rect x="{-w/2 + 13 + i*28}" y="{-h/2 + 9}" width="22" height="14" rx="2" fill="{ui}" opacity="{op}"/>')
        g.append(f'<rect x="{-w/2 + 13}" y="{-h/2 + 30}" width="50" height="5" rx="2.5" fill="{ui}" opacity="{op}"/>')
        g.append(f'<path d="M-22 {h/2 + 6} h44 M0 {h/2 - 4} v10" stroke="{edge}" stroke-width="3.5" stroke-linecap="round"/>')
    else:
        w, h = 60, 112
        dash = ' stroke-dasharray="7 5"' if emu else ''
        g.append(f'<rect x="{w/2 - 1}" y="-24" width="4" height="20" rx="2" fill="{edge}"/>')
        g.append(f'<rect x="{-w/2}" y="{-h/2}" width="{w}" height="{h}" rx="13" fill="{TERM}" stroke="{edge}" stroke-width="3.5"{dash}/>')
        g.append(f'<rect x="{-w/2 + 5}" y="{-h/2 + 5}" width="{w - 10}" height="{h - 10}" rx="8" fill="{screen}"/>')
        g.append(f'<circle cx="0" cy="{-h/2 + 13}" r="3" fill="{TERM}"/>')
        # a small app layout: title bar, a card, two lines, an action button
        g.append(f'<rect x="-19" y="{-h/2 + 23}" width="26" height="5" rx="2.5" fill="{ui}" opacity="{op}"/>')
        g.append(f'<rect x="-19" y="{-h/2 + 34}" width="38" height="22" rx="4" fill="{ui}" opacity="{op}"/>')
        g.append(f'<rect x="-19" y="{-h/2 + 62}" width="38" height="4" rx="2" fill="{ui}" opacity="{op}"/>')
        g.append(f'<rect x="-19" y="{-h/2 + 71}" width="24" height="4" rx="2" fill="{ui}" opacity="{op}"/>')
        g.append(f'<circle cx="13" cy="{h/2 - 18}" r="6" fill="{ui}" opacity="{op}"/>')
        g.append(f'<rect x="-9" y="{h/2 - 10}" width="18" height="2.4" rx="1.2" fill="{ui}" opacity="{op}"/>')
    g.append('</g>')
    return "".join(g)

def lock(cx, cy):
    """A padlock on the line: this session holds the device."""
    return (f'<circle cx="{cx}" cy="{cy}" r="15" fill="{TAPE}"/>'
            f'<path d="M{cx-4.5} {cy-2} v-3.5 a4.5 4.5 0 0 1 9 0 v3.5" fill="none" stroke="{TAPEINK}" stroke-width="2.4" stroke-linecap="round"/>'
            f'<rect x="{cx-7}" y="{cy-2.5}" width="14" height="10.5" rx="2.5" fill="{TAPEINK}"/>')

def tag(x, y, text, kind):
    w = len(text) * 9.7 + 20
    if kind == "held":
        return (f'<g transform="rotate(-2 {x} {y})"><rect x="{x+2}" y="{y+3}" width="{w}" height="28" rx="3" fill="{INK}"/>'
                f'<rect x="{x}" y="{y}" width="{w}" height="28" rx="3" fill="{TAPE}" stroke="{TAPEINK}" stroke-width="1.5"/>'
                + t(x + 10, y + 19.5, text, 16, TAPEINK, MONO, 700) + '</g>')
    col = FREE if kind == "free" else NO
    return (f'<rect x="{x}" y="{y}" width="{w}" height="28" rx="3" fill="none" stroke="{col}" stroke-width="1.5" stroke-dasharray="5 4"/>'
            + t(x + 10, y + 19.5, text, 16, col, MONO, 700))

def device(cx, cy, model, serial, tagtext, kind, tv=False, bad=False):
    tx = cx + 70
    return phone(cx, cy, kind == "held", tv, bad, model == "Emulator") + t(tx, cy - 22, model, 25, INK, BODY, 600) + t(tx, cy + 1, serial, 15, MUTED, MONO) + tag(tx, cy + 14, tagtext, kind)

def arrow(x1, y, x2, col=TAPE):
    return (f'<line x1="{x1}" y1="{y}" x2="{x2 - 12}" y2="{y}" stroke="{col}" stroke-width="3.5" stroke-linecap="round"/>'
            f'<path d="M{x2} {y} l-16 -9 v18 z" fill="{col}"/>')

def cross(x, y, r=15):
    return (f'<circle cx="{x}" cy="{y}" r="{r}" fill="{BENCH}" stroke="{NO}" stroke-width="3"/>'
            f'<path d="M{x-6} {y-6} l12 12 M{x+6} {y-6} l-12 12" stroke="{NO}" stroke-width="3" stroke-linecap="round"/>')

def svg(w, h, body, title):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-label="{title}">'
            f'<rect width="{w}" height="{h}" fill="{BENCH}"/>{body}</svg>\n')

def how(H=900, rows=(300, 440, 580, 720), ah=108, foot=True):
    global SCALE
    SCALE = 1.0 if foot else 0.86
    b = [t(80, 108, "adbrent", 46, TAPE, DISP, 800), t(80, 160, "Each agent session gets its own Android device. Anyone else is refused.", 28, INK, BODY, 400)]
    ax, aw = 80, 380
    gx, gw = 850, 76
    dcx = 1190
    names = [("shop-app", "adb install app.apk"), ("chat-app", "adb shell am instrument"), ("maps-app", "adb logcat")]
    for (name, cmd), y in zip(names, rows):
        b.append(arrow(ax + aw, y, dcx - 50))
    # the refused attempt: chat-app reaches for the Pixel 8, and stops at adbrent
    y0, y1 = rows[1] - 26, rows[0] + 40
    b.append(f'<path d="M{ax+aw} {y0} C{ax+aw+130} {y0} {gx-140} {y1} {gx-16} {y1}" fill="none" stroke="{NO}" stroke-width="3" stroke-dasharray="9 8" stroke-linecap="round"/>')
    top, bot = rows[0] - 78, rows[-1] + 78
    b.append(f'<rect x="{gx}" y="{top}" width="{gw}" height="{bot-top}" rx="14" fill="{SURF}" stroke="{SHELF}" stroke-width="2"/>')
    b.append(t(gx + gw/2, top - 14, "adbrent", 18, TAPE, MONO, 700, "middle"))
    b.append(t(gx + gw/2, bot + 28, "stands in for adb", 15, MUTED, BODY, 400, "middle"))
    for y in rows[:3]:
        b.append(lock(gx + gw/2, y))
    b.append(cross(gx - 2, y1))
    if foot:
        b.append(t(gx - 26, y1 + 46, "refused", 16, NO, MONO, 700, "end"))
        b.append(t(gx - 26, y1 + 68, "rented by shop-app", 15.5, NO, MONO, 400, "end"))
    else:
        b.append(t(gx - 26, y1 + 48, "refused", 17, NO, MONO, 700, "end"))
    for (name, cmd), y in zip(names, rows):
        b.append(agent(ax, y - ah/2, aw, ah, name, cmd))
    devs = [("Pixel 8", "3A021FDJH000XY", "held by shop-app", "held", False), ("Galaxy S23", "R5CT20ABCDE", "held by chat-app", "held", False),
            ("Emulator", "emulator-5554", "held by maps-app", "held", False), ("Chromecast", "GTV0001", "free", "free", True)]
    for (m, s, tg, k, tv), y in zip(devs, rows):
        b.append(device(dcx, y, m, s, tg, k, tv))
    if foot:
        b.append(t(80, H - 42, "Released the moment the session ends. No daemon, no setup per agent.", 20, MUTED, BODY))
        b.append(t(1520, H - 42, "github.com/iammosab/adbrent", 18, MUTED, MONO, 400, "end"))
    return svg(1600, H, "".join(b), "Three agent sessions each reach their own Android device through adbrent; a second attempt on a held device is refused.")

def panel(x, y, w, h, heading, col):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="18" fill="{SURF}" stroke="{LINE}" stroke-width="2"/>' + t(x + 36, y + 58, heading, 30, col, DISP, 800)

def before_after():
    global SCALE
    SCALE = 1.0
    b = [t(80, 108, "adbrent", 46, TAPE, DISP, 800), t(80, 160, "Two agents, two phones. The difference is who gets which.", 28, INK, BODY)]
    pw, ph, py = 700, 600, 210
    for i, (heading, col) in enumerate([("Without adbrent", NO), ("With adbrent", TAPE)]):
        px = 80 + i * (pw + 40)
        b.append(panel(px, py, pw, ph, heading, col))
        ax, aw, ah = px + 36, 260, 100
        ys = [py + 190, py + 350]
        dcx = px + 430
        if i == 0:
            # both agents land on the first phone
            for y in ys:
                b.append(f'<path d="M{ax+aw} {y} C{ax+aw+70} {y} {dcx-110} {ys[0]} {dcx-50} {ys[0]}" fill="none" stroke="{NO}" stroke-width="3.5" stroke-linecap="round"/>')
            b.append(f'<path d="M{dcx-34} {ys[0]} l-16 -9 v18 z" fill="{NO}"/>')
            b.append(device(dcx, ys[0], "Pixel 8", "3A021FDJH000XY", "used by both", "bad", False, True))
            b.append(device(dcx, ys[1], "Galaxy S23", "R5CT20ABCDE", "idle", "free"))
            cap = ["shop-app installs a new build while", "chat-app is halfway through a UI test."]
        else:
            for y in ys:
                b.append(arrow(ax + aw, y, dcx - 50))
            b.append(device(dcx, ys[0], "Pixel 8", "3A021FDJH000XY", "held by shop-app", "held"))
            b.append(device(dcx, ys[1], "Galaxy S23", "R5CT20ABCDE", "held by chat-app", "held"))
            cap = ["Each session holds one device until it ends.", "Neither agent was told anything; both typed adb."]
        for (name, cmd), y in zip([("shop-app", "adb install"), ("chat-app", "adb shell")], ys):
            b.append(agent(ax, y - ah/2, aw, ah, name, cmd, ""))
        b.append(t(px + 36, py + ph - 78, cap[0], 22, INK, BODY))
        b.append(t(px + 36, py + ph - 46, cap[1], 22, INK, BODY))
    b.append(t(1520, 858, "github.com/iammosab/adbrent", 18, MUTED, MONO, 400, "end"))
    return svg(1600, 900, "".join(b), "Without adbrent two agents use the same phone; with adbrent each holds its own.")

for name, s in (("how-it-works", how()), ("before-after", before_after()), ("social", how(800, (286, 404, 522, 640), 96, False))):
    open(os.path.join(OUT, name + ".svg"), "w").write(s)
    open(os.path.join(OUT, name + ".html"), "w").write('<!doctype html><meta charset="utf-8"><link href="https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,500..800&family=Instrument+Sans:wght@400..600&family=JetBrains+Mono:wght@400;700&display=swap" rel="stylesheet"><style>html,body{margin:0;background:#0E161C}svg{display:block}</style>' + s)
print("ok")
