#!/usr/bin/env python3
# Experiment, not for merging: summarizes QEMU's sdhci traces of the
# benchmark boots (OB_SD_TRACE) per data command size.
import collections, re, sys, pathlib

LINE = re.compile(r"^\d+@(\d+)\.(\d+):(\w+) ?(.*)$")
DATA = {17: "read1", 18: "read", 24: "write1", 25: "write"}

def summarize(path):
    rows = collections.defaultdict(lambda: collections.defaultdict(float))
    cur = None
    last_end = None
    between = collections.Counter()
    for line in open(path, errors="replace"):
        m = LINE.match(line)
        if not m:
            continue
        t = int(m.group(1)) * 1_000_000 + int(m.group(2))
        ev, args = m.group(3), m.group(4)
        if ev == "sdhci_send_command":
            cmd = int(args[3:5])
            if cmd in DATA:
                gap = (t - last_end) if last_end is not None else None
                cur = {"cmd": DATA[cmd], "start": t, "descs": [], "gap": gap,
                       "between": dict(between)}
                between.clear()
            else:
                between[f"cmd{cmd}"] += 1
        elif ev == "sdhci_adma_loop" and cur is not None:
            ln = int(re.search(r"len=(\d+)", args).group(1)) or 65536
            cur["descs"].append((t, ln))
        elif ev == "sdhci_adma_transfer_completed" and cur is not None:
            total = sum(l for _, l in cur["descs"])
            key = (cur["cmd"], total)
            r = rows[key]
            r["n"] += 1
            r["descs"] += len(cur["descs"])
            r["dur"] += t - cur["start"]
            # time from the command to the first descriptor, and the
            # largest step between descriptors (a batch boundary)
            if cur["descs"]:
                r["first"] += cur["descs"][0][0] - cur["start"]
                steps = [b[0] - a[0] for a, b in zip(cur["descs"], cur["descs"][1:])]
                r["maxstep"] += max(steps) if steps else 0
            if cur["gap"] is not None and cur["gap"] < 1_000_000:
                r["gap"] += cur["gap"]
                r["gaps"] += 1
            for k, v in cur["between"].items():
                r[k] += v
            last_end = t
            cur = None
        elif ev == "sdhci_access":
            between["read" if args.startswith("rd") else "write"] += 1
    return rows

def main():
    for path in sorted(pathlib.Path(sys.argv[1]).glob("*.log")):
        rows = summarize(path)
        print(f"== {path.name}")
        print("op      bytes     n  descs  dur_us  us/KiB  first_us maxstep_us  gap_us  reg_r  reg_w  other")
        for (op, total), r in sorted(rows.items(), key=lambda kv: (kv[0][1], kv[0][0])):
            n = r["n"]
            if n < 20:
                continue
            g = r["gaps"] or 1
            other = " ".join(f"{k}={r[k]/n:.2f}" for k in sorted(r) if k.startswith("cmd"))
            print(f"{op:6} {total:7} {int(n):5} {r['descs']/n:6.1f} {r['dur']/n:7.0f} "
                  f"{r['dur']/n/(total/1024):7.1f} {r['first']/n:8.0f} {r['maxstep']/n:9.0f} "
                  f"{r['gap']/g:7.0f} {r['read']/n:6.1f} {r['write']/n:6.1f}  {other}")

main()
