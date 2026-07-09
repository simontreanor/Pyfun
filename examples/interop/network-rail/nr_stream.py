"""Streaming boundary for the Network Rail all-TOC SCHEDULE snapshot.

Used by chippenham_fast.pyfun. The whole feed stays here: we read the NDJSON one
line at a time (gzip-aware) in a native Python loop, skip lines with a cheap
substring test *before* paying for json.loads, and hand back only the small set
of schedules that mention Chippenham. Pyfun owns the domain logic.

This is the *fast* boundary: a tight C-level `for` loop over ~20M lines is far
quicker than reducing them one Pyfun call at a time. chippenham.pyfun shows the
same job done with no Python at all — slower, and that contrast is the point.
"""

import gzip
import json
import os

CHIPP = "CHIPNHM"          # Chippenham (Wiltshire) TIPLOC (CRS: CPM)


def write_utf8(path, text):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    return len(text)


def _resolve(path):
    # Accept either the given file or a .gz alongside it, without editing Pyfun.
    if os.path.exists(path):
        return path
    if os.path.exists(path + ".gz"):
        return path + ".gz"
    return path


def _open(path):
    return (gzip.open(path, "rt", encoding="utf-8")
            if path.endswith(".gz")
            else open(path, "rt", encoding="utf-8"))


def scan(path):
    """Single streaming pass -> (tiploc_name_map, chippenham_runs).

    chippenham_runs: list of (uid, days, stp, stops)
    stops:           list of (tiploc, public_arrival, public_departure)
    """
    names = {}
    runs = []
    path = _resolve(path)
    with _open(path) as f:
        for line in f:
            # Substring prefilter: the vast majority of schedule lines never
            # mention Chippenham, so we skip json.loads on them entirely.
            if '"TiplocV1"' in line:
                t = json.loads(line)["TiplocV1"]
                code = t.get("tiploc_code")
                if code:
                    names[code] = (t.get("tps_description")
                                   or t.get("description") or code)
            elif '"JsonScheduleV1"' in line and ('"' + CHIPP + '"') in line:
                s = json.loads(line)["JsonScheduleV1"]
                if s.get("transaction_type") == "Delete":
                    continue
                seg = s.get("schedule_segment") or {}
                locs = seg.get("schedule_location") or []
                if not any(l.get("tiploc_code") == CHIPP for l in locs):
                    continue  # substring matched elsewhere (e.g. a name); confirm
                stops = [(l.get("tiploc_code") or "",
                          l.get("public_arrival") or "",
                          l.get("public_departure") or "")
                         for l in locs]
                runs.append((s.get("CIF_train_uid") or "",
                             s.get("schedule_days_runs") or "",
                             s.get("CIF_stp_indicator") or "",
                             stops))
    return (names, runs)
