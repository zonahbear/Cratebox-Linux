#!/usr/bin/env python3
"""The live layer carries its own copy of /var/lib/dpkg/status, which hides the one in the layer we
changed. Rebuild it: the new installed-system status, plus what the live layer added on top of the
original, minus what it removed.
  merge-dpkg-status.py ORIG_STANDARD NEW_STANDARD ORIG_LIVE OUT"""
import sys

def stanzas(path):
    out = {}
    for block in open(path, encoding="utf-8").read().split("\n\n"):
        if not block.strip():
            continue
        f = dict(l.split(":", 1) for l in block.splitlines() if l and not l[0].isspace() and ":" in l)
        out[(f["Package"].strip(), f.get("Architecture", "").strip())] = block.strip("\n")
    return out

orig_std, new_std, orig_live = (stanzas(p) for p in sys.argv[1:4])
added = {k: v for k, v in orig_live.items() if k not in orig_std}
removed = {k for k in orig_std if k not in orig_live}
merged = {k: v for k, v in new_std.items() if k not in removed}
merged.update(added)
with open(sys.argv[4], "w", encoding="utf-8") as f:
    f.write("\n\n".join(merged[k] for k in sorted(merged)) + "\n")
print(f"live dpkg status: {len(merged)} packages ({len(added)} live-only, {len(removed)} removed by the live layer)")
