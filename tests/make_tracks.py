"""Synthetic test tracks with known tempo and first-beat offset."""
import numpy as np, wave, os, sys
SR = 44100
rng = np.random.default_rng(7)

def kick(n=int(0.35*SR)):
    t = np.arange(n)/SR
    f = 45 + 110*np.exp(-t*30)
    return np.sin(2*np.pi*np.cumsum(f)/SR) * np.exp(-t*7)
def snare(n=int(0.2*SR)):
    t = np.arange(n)/SR
    return (rng.standard_normal(n)*0.6 + 0.4*np.sin(2*np.pi*190*t)) * np.exp(-t*18)
def hat(n=int(0.05*SR)):
    return rng.standard_normal(n) * np.exp(-np.arange(n)/SR*90) * 0.25

def place(buf, s, at):
    i = int(at*SR)
    if i >= len(buf): return
    e = min(len(buf), i+len(s)); buf[i:e] += s[:e-i]

MAJ = [0, 2, 4, 5, 7, 9, 11]; MIN = [0, 2, 3, 5, 7, 8, 10]
def harmony(n, bpm, offset, tonic, minor):
    """Pads and a bass line in a known key: i-VI-III-VII (minor) or I-V-vi-IV (major), a chord per bar."""
    scale = [tonic + d for d in (MIN if minor else MAJ)]
    prog = [0, 5, 2, 6] if minor else [0, 4, 5, 3]
    bar = 4 * 60 / bpm; y = np.zeros(n)
    k = 0
    while offset + k * bar < n / SR:
        deg = prog[k % 4]; i0 = int((offset + k * bar) * SR); L = min(int(bar * SR), n - i0)
        t = np.arange(L) / SR
        env = np.minimum(1, t / 0.02) * np.minimum(1, (bar - t) / 0.05)
        for j in (0, 2, 4):
            m = 60 + scale[(deg + j) % 7] + 12 * ((deg + j) // 7)
            if m > 74: m -= 12
            f = 440 * 2 ** ((m - 69) / 12)
            y[i0:i0 + L] += 0.035 * env * sum(np.sin(2 * np.pi * f * h * t) / h for h in range(1, 6))
        f = 440 * 2 ** ((36 + scale[deg] % 12 - 69) / 12)
        y[i0:i0 + L] += 0.14 * env * (np.sin(2 * np.pi * f * t) + 0.3 * np.sin(4 * np.pi * f * t))
        k += 1
    return y

def track(bpm, offset, pattern, secs=75, key=(9, True), layout=None):
    """layout: [(first_bar, end_bar, drums_on, harmony_on)] for tracks with sections"""
    n = int(secs*SR); x = np.zeros(n)
    spb = 60/bpm; beat = 0
    t = offset
    def on(bar, idx):
        if not layout: return True
        return any(a <= bar < b and l[idx] for (a, b, *l) in layout)
    while t < secs:
        if not on(beat // 4, 0):
            beat += 1; t += spb; continue
        bar_pos = beat % 4
        for kind, positions in pattern.items():
            for p in positions:
                if abs(p - bar_pos) < 1e-9 or (bar_pos == int(p) and False): pass
        for p in pattern.get("kick", []):
            if beat % 4 == 0: place(x, kick(), t + p*spb)
        for p in pattern.get("snare", []):
            if beat % 4 == 0: place(x, snare()*0.8, t + p*spb)
        for p in pattern.get("hat", []):
            if beat % 4 == 0: place(x, hat(), t + p*spb)
        beat += 1; t += spb
    hm = harmony(n, bpm, offset, *key)
    if layout:
        bar_len = 4 * spb
        for b in range(int((secs - offset) / bar_len) + 1):
            if not on(b, 1):
                i0 = int((offset + b * bar_len) * SR); hm[i0:i0 + int(bar_len * SR)] = 0
    x = x + hm
    x /= np.max(np.abs(x)) * 1.12
    st = np.stack([x, x*0.97], axis=1)
    return (st*32767).astype(np.int16)

# keys: A minor = 8A, F minor = 4A, D minor = 7A, E-flat major = 5B
TRACKS = [
    # sections: intro bars 0-7 drums only, drop 8-19, breakdown 20-31 (no drums), drop from 32
    ("Test Artist - House Groove 126.wav", 126, 0.37, {"kick": [0,1,2,3], "snare": [1,3], "hat": [0.5,1.5,2.5,3.5]}, (9, True),
     [(0, 8, True, False), (8, 20, True, True), (20, 32, False, True), (32, 99, True, True)]),
    ("Bass Lab - Halftime Wobble 140.wav", 140, 0.05, {"kick": [0, 2.5], "snare": [2], "hat": [0,0.5,1,1.5,2,2.5,3,3.5]}, (5, True), None),
    ("Jungle Unit - Rollers 174.wav", 174, 0.21, {"kick": [0, 1.5], "snare": [1, 3], "hat": [0.5,1.5,2.5,3.5]}, (2, True), None),
    ("Boom Bap Crew - Dusty 92.wav", 92, 0.5, {"kick": [0, 1.75, 2.5], "snare": [1, 3], "hat": [0,0.5,1,1.5,2,2.5,3,3.5]}, (3, False), None),
]
out = sys.argv[1]
for name, bpm, off, pat, key, layout in TRACKS:
    data = track(bpm, off, pat, key=key, layout=layout)
    with wave.open(os.path.join(out, name), "wb") as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(SR); w.writeframes(data.tobytes())
    print(name, bpm, off)
