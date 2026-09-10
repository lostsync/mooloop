"""Flatten the measurement JSON into tidy tables a chart can be built from.

Every file in `out/tidy/` is long-format: one row per measured point, with
the unit, the setting and the axis values in their own columns, so a chart is
a filter and a group-by rather than a reshaping exercise. Nothing here
measures anything; it reads `out/chart/` and derives.

Two derivations are worth knowing about because they are the difference
between a plottable number and a misleading one.

**EQ curves are referenced to the unit's own flat curve, not to 1 kHz.** A
bell at 400 Hz lifts 1 kHz by several dB, so normalising each curve at 1 kHz
subtracts part of the boost being measured -- an API 550A's +12 marking reads
as +7 dB that way and as +11.9 dB against flat. Each sweep records its own
`ref_1k_db`, so the absolute curve is recoverable and the subtraction is
exact.

**Headroom is quoted as the input level that produces 1% THD**, interpolated
between the measured levels. It is the one number that puts a console
channel, a tape machine and a clipper on the same axis, and it is the axis a
studio reader is actually asking about when they ask how hard a thing can be
hit.
"""

import csv
import glob
import json
import math
import os
import sys

OUT = "out/tidy"


def _rows(path, fields, rows):
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, path), "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=fields, extrasaction="ignore")
        w.writeheader()
        for row in rows:
            w.writerow(row)
    print("%-32s %6d rows" % (path, len(rows)))


def load(family):
    out = {}
    for f in sorted(glob.glob("out/chart/%s/*.json" % family)):
        d = json.load(open(f))
        if "error" in d:
            continue
        out[d["slug"]] = d
    return out


def meta_of(d):
    return {k: d.get(k) for k in
            ("slug", "maker", "models", "family", "group")}


def interp_level_at(points, target):
    """The input level at which a rising quantity first reaches `target`.

    `points` is (level_dbfs, value) sorted by level. Linear in dB against
    log(value), which is how distortion actually grows, and returns None
    rather than extrapolating past what was measured.
    """
    pts = [(lv, v) for lv, v in sorted(points) if v and v > 0]
    for (l0, v0), (l1, v1) in zip(pts, pts[1:]):
        if v0 < target <= v1:
            span = math.log10(v1) - math.log10(v0)
            if span <= 0:
                return l1
            f = (math.log10(target) - math.log10(v0)) / span
            return round(l0 + f * (l1 - l0), 2)
    return None


def curve_abs(sweep):
    """A sweep's magnitude in absolute terms, undoing the 1 kHz reference."""
    if not sweep:
        return {}
    ref = sweep.get("ref_1k_db", 0.0)
    return {k: v + ref for k, v in sweep["magnitude_db"].items()}


def bell_shape(rel):
    """Peak height, peak frequency and Q of a curve given relative to flat."""
    if not rel:
        return {}
    keys = sorted(rel, key=float)
    peak_key = max(keys, key=lambda k: abs(rel[k]))
    peak = rel[peak_key]
    f0 = float(peak_key)
    if abs(peak) < 0.5:
        return {"peak_db": round(peak, 3), "peak_hz": f0, "q": None,
                "bandwidth_oct": None}
    half = peak / 2.0
    i = keys.index(peak_key)

    def edge(step):
        j = i
        while 0 <= j + step < len(keys):
            j += step
            if abs(rel[keys[j]]) < abs(half):
                a, b = keys[j - step], keys[j]
                va, vb = rel[a], rel[b]
                if va == vb:
                    return float(b)
                t = (half - va) / (vb - va)
                return float(a) * (float(b) / float(a)) ** t
        return None

    lo, hi = edge(-1), edge(1)
    if not lo or not hi or hi <= lo:
        return {"peak_db": round(peak, 3), "peak_hz": f0, "q": None,
                "bandwidth_oct": None}
    return {"peak_db": round(peak, 3), "peak_hz": f0,
            "q": round(f0 / (hi - lo), 3),
            "bandwidth_oct": round(math.log2(hi / lo), 3)}


# ------------------------------------------------------------------ colour


def export_colour():
    units = load("colour")
    resp, thd, tilt, imd, noise, trans, summary = [], [], [], [], [], [], []
    wow, tone_resp = [], []

    for slug, d in sorted(units.items()):
        meta = meta_of(d)
        for label, s in sorted(d.get("settings", {}).items()):
            base = dict(meta, setting=label, depth=s.get("depth"),
                        matched=s.get("matched"))

            for level, sweep in (s.get("response") or {}).items():
                if not sweep:
                    continue
                agree = (s.get("response_agreement") or {}).get(level) or {}
                for hz, db in sweep["magnitude_db"].items():
                    resp.append(dict(
                        base, level_dbfs=float(level), hz=float(hz),
                        magnitude_db=db,
                        phase_deg=sweep["phase_deg"].get(hz),
                        group_delay_ms=sweep["group_delay_ms"].get(hz),
                        ref_1k_db=sweep.get("ref_1k_db"),
                        sweep_check_worst_db=agree.get("worst_db")))

            for key, row in (s.get("thd_vs_level") or {}).items():
                hz, level = key.split("/")
                thd.append(dict(
                    base, hz=float(hz), level_dbfs=float(level),
                    thd_pct=row.get("thd_pct"), thd_db=row.get("thd_db"),
                    gain_db=row.get("gain_db"),
                    noise_floor_db=row.get("noise_floor_db"),
                    clipped=row.get("clipped"),
                    rejected_runs=row.get("rejected_runs"),
                    **{"h%d" % m: row.get("h%d" % m) for m in range(2, 9)}))

            # A tone-measured response beside the swept one. A swept sine
            # deconvolves the *linear* part of what a unit does, which is the
            # response only while the unit is linear -- and several here are
            # not, at any level. `sweep_check_worst_db` says which; this is
            # what to plot for those, since a tone's fundamental is the
            # fundamental whatever else the stage is doing to it.
            for level, row in (s.get("response_check") or {}).items():
                ref = row.get("1000")
                for hz, db in row.items():
                    tone_resp.append(dict(
                        base, source="check", level_dbfs=float(level),
                        hz=float(hz), gain_db=db,
                        magnitude_db=(round(db - ref, 3)
                                      if ref is not None else None)))

            for key, row in (s.get("harmonics_vs_freq") or {}).items():
                hz, level = key.split("/")
                tilt.append(dict(
                    base, hz=float(hz), level_dbfs=float(level),
                    thd_pct=row.get("thd_pct"),
                    valid_orders=row.get("valid_orders"),
                    **{"h%d" % m: row.get("h%d" % m) for m in range(2, 9)}))

            grid = s.get("harmonics_vs_freq") or {}
            for level in {k.split("/")[1] for k in grid}:
                ref = (grid.get("1000/%s" % level) or {}).get("gain_db")
                for key, row in grid.items():
                    hz, lv = key.split("/")
                    if lv != level:
                        continue
                    db = row.get("gain_db")
                    tone_resp.append(dict(
                        base, source="tilt_grid", level_dbfs=float(lv),
                        hz=float(hz), gain_db=db,
                        magnitude_db=(round(db - ref, 3)
                                      if None not in (db, ref) else None)))

            for test, rows in (s.get("imd") or {}).items():
                for row in rows or []:
                    if not row:
                        continue
                    imd.append(dict(base, test=test,
                                    level_dbfs=row.get("peak_in_dbfs"),
                                    imd_pct=row.get("imd_pct"),
                                    imd_db=row.get("imd_db")))

            for cond, row in (s.get("noise") or {}).items():
                if not row:
                    continue
                for hz, db in (row.get("third_octave_dbfs") or {}).items():
                    noise.append(dict(base, condition=cond, hz=float(hz),
                                      level_dbfs=db,
                                      rms_dbfs=row.get("rms_dbfs"),
                                      a_weighted_dbfs=row.get("a_weighted_dbfs")))

            for level, row in (s.get("transients") or {}).items():
                trans.append(dict(base, level_dbfs=float(level), **row))

            wf = s.get("wow_flutter")
            if wf:
                for hz, pct in (wf.get("modulation_spectrum_pct")
                                or {}).items():
                    wow.append(dict(base, rate_hz=float(hz),
                                    deviation_pct=pct,
                                    measured_hz=wf.get("measured_hz"),
                                    speed_error_pct=wf.get("speed_error_pct"),
                                    wow_pct=wf.get("wow_pct_0p1_4hz"),
                                    flutter_pct=wf.get("flutter_pct_4_100hz"),
                                    scrape_pct=wf.get("scrape_pct_100_1000hz"),
                                    total_rms_pct=wf.get("total_rms_pct"),
                                    peak_pct=wf.get("peak_pct")))

            summary.append(_colour_summary(base, s, d))

    _rows("colour_response.csv",
          ["slug", "maker", "models", "family", "group", "setting", "depth",
           "matched", "level_dbfs", "hz", "magnitude_db", "phase_deg",
           "group_delay_ms", "ref_1k_db", "sweep_check_worst_db"], resp)
    _rows("colour_response_by_tone.csv",
          ["slug", "maker", "models", "family", "group", "setting", "depth",
           "matched", "source", "level_dbfs", "hz", "gain_db",
           "magnitude_db"], tone_resp)
    _rows("colour_thd_vs_level.csv",
          ["slug", "maker", "models", "group", "setting", "depth", "matched",
           "hz", "level_dbfs", "thd_pct", "thd_db", "gain_db",
           "noise_floor_db", "clipped", "rejected_runs"]
          + ["h%d" % m for m in range(2, 9)], thd)
    _rows("colour_harmonics_vs_freq.csv",
          ["slug", "maker", "models", "group", "setting", "depth", "matched",
           "hz", "level_dbfs", "thd_pct", "valid_orders"]
          + ["h%d" % m for m in range(2, 9)], tilt)
    _rows("colour_imd.csv",
          ["slug", "maker", "models", "group", "setting", "depth", "matched",
           "test", "level_dbfs", "imd_pct", "imd_db"], imd)
    _rows("colour_noise.csv",
          ["slug", "maker", "models", "group", "setting", "condition", "hz",
           "level_dbfs", "rms_dbfs", "a_weighted_dbfs"], noise)
    _rows("colour_transients.csv",
          ["slug", "maker", "models", "group", "setting", "level_dbfs",
           "crest_in_db", "crest_out_db", "crest_change_db", "slew_index",
           "click_peak_gain_db", "tone_gain_db", "square_slope_ratio",
           "sine_slope_ratio"], trans)
    _rows("tape_wow_flutter.csv",
          ["slug", "maker", "models", "group", "setting", "rate_hz",
           "deviation_pct", "measured_hz", "speed_error_pct", "wow_pct",
           "flutter_pct", "scrape_pct", "total_rms_pct", "peak_pct"], wow)
    _rows("colour_summary.csv",
          ["slug", "maker", "models", "family", "group", "setting", "depth",
           "matched", "gain_1k_db", "thd_1k_pct", "thd_100_pct",
           "headroom_1pct_dbfs", "headroom_3pct_dbfs", "h2_80", "h3_80",
           "h2_minus_h3_80", "h2_5k", "tilt_db", "imd_smpte_pct",
           "imd_ccif_pct", "noise_a_dbfs", "noise_under_tone_dbfs",
           "crest_change_12_db", "crest_change_3_db", "slew_index_3",
           "lf_shelf_db", "hf_shelf_db", "wow_pct", "flutter_pct",
           "speed_error_pct"], summary)
    return units


def _colour_summary(base, s, d):
    """One row per unit and setting: the numbers a table would quote."""
    row = dict(base)
    lv = s.get("thd_vs_level") or {}

    def at(hz, level, field="thd_pct"):
        return (lv.get("%g/%g" % (hz, level)) or {}).get(field)

    row["thd_1k_pct"] = at(1000, -12)
    row["thd_100_pct"] = at(100, -12)
    row["gain_1k_db"] = at(1000, -12, "gain_db")
    series = [(float(k.split("/")[1]), v.get("thd_pct"))
              for k, v in lv.items() if k.startswith("1000/")]
    row["headroom_1pct_dbfs"] = interp_level_at(series, 1.0)
    row["headroom_3pct_dbfs"] = interp_level_at(series, 3.0)

    tilt = s.get("harmonics_vs_freq") or {}

    def h(hz, order, level=-12.0):
        return (tilt.get("%g/%g" % (hz, level)) or {}).get("h%d" % order)

    row["h2_80"], row["h3_80"] = h(80, 2), h(80, 3)
    if row["h2_80"] is not None and row["h3_80"] is not None:
        row["h2_minus_h3_80"] = round(row["h2_80"] - row["h3_80"], 2)
    row["h2_5k"] = h(5000, 2)
    if row["h2_80"] is not None and row["h2_5k"] is not None:
        # The "warm on a kick, clean on a hat" number: how much further down
        # the 2nd harmonic is at 5 kHz than at 80 Hz.
        row["tilt_db"] = round(row["h2_80"] - row["h2_5k"], 2)

    for test, key in (("smpte_60_7k", "imd_smpte_pct"),
                      ("ccif_19_20k", "imd_ccif_pct")):
        for r in (s.get("imd") or {}).get(test) or []:
            if r and r.get("peak_in_dbfs") == -12.0:
                row[key] = r.get("imd_pct")

    noise = s.get("noise") or {}
    row["noise_a_dbfs"] = (noise.get("silence") or {}).get("a_weighted_dbfs")
    row["noise_under_tone_dbfs"] = (
        noise.get("under_tone_-12") or {}).get("a_weighted_dbfs")

    tr = s.get("transients") or {}
    row["crest_change_12_db"] = (tr.get("-12") or {}).get("crest_change_db")
    row["crest_change_3_db"] = (tr.get("-3") or {}).get("crest_change_db")
    row["slew_index_3"] = (tr.get("-3") or {}).get("slew_index")

    # Response shape at the operating level, as two numbers: what the stage
    # does to the bottom and the top relative to 1 kHz.
    sweep = (s.get("response") or {}).get("-12")
    if sweep:
        mag = sweep["magnitude_db"]
        keys = sorted(mag, key=float)
        near = lambda f: mag[min(keys, key=lambda k: abs(float(k) - f))]
        row["lf_shelf_db"] = round(near(50.0), 3)
        row["hf_shelf_db"] = round(near(12000.0), 3)

    wf = s.get("wow_flutter")
    if wf:
        row["wow_pct"] = wf.get("wow_pct_0p1_4hz")
        row["flutter_pct"] = wf.get("flutter_pct_4_100hz")
        row["speed_error_pct"] = wf.get("speed_error_pct")
    return row


# ---------------------------------------------------------------------- EQ


def export_eq():
    units = load("eq")
    curves, shapes, dist, wow = [], [], [], []

    for slug, d in sorted(units.items()):
        meta = meta_of(d)
        flat = curve_abs((d.get("flat") or {}).get("sweep"))
        if not flat:
            continue

        for key, row in ((d.get("flat") or {}).get("distortion") or {}).items():
            hz, level = key.split("/")
            dist.append(dict(meta, band="flat", gain_label="flat",
                             hz=float(hz), level_dbfs=float(level),
                             thd_pct=row.get("thd_pct"),
                             **{"h%d" % m: row.get("h%d" % m)
                                for m in range(2, 9)}))

        for band, b in sorted((d.get("bands") or {}).items()):
            for label, g in b["gains"].items():
                absolute = curve_abs(g.get("sweep"))
                if not absolute:
                    continue
                rel = {k: round(absolute[k] - flat[k], 3)
                       for k in absolute if k in flat}
                for hz, db in rel.items():
                    curves.append(dict(
                        meta, band=band, target_hz=b.get("target_hz"),
                        actual_hz=b.get("actual_hz"), marking=b.get("marking"),
                        gain_label=label, hz=float(hz), gain_db=db))
                shapes.append(dict(
                    meta, band=band, target_hz=b.get("target_hz"),
                    actual_hz=b.get("actual_hz"), marking=b.get("marking"),
                    gain_label=label, **bell_shape(rel)))

            for key, row in ((b.get("distortion") or {}).get("rows")
                             or {}).items():
                hz, level = key.split("/")
                dist.append(dict(
                    meta, band=band, gain_label=b["distortion"].get("label"),
                    hz=float(hz), level_dbfs=float(level),
                    thd_pct=row.get("thd_pct"),
                    **{"h%d" % m: row.get("h%d" % m) for m in range(2, 9)}))

        for label, e in sorted((d.get("extra") or {}).items()):
            absolute = curve_abs(e.get("sweep"))
            if absolute:
                rel = {k: round(absolute[k] - flat[k], 3)
                       for k in absolute if k in flat}
                for hz, db in rel.items():
                    curves.append(dict(meta, band="extra", gain_label=label,
                                       hz=float(hz), gain_db=db))
                shapes.append(dict(meta, band="extra", gain_label=label,
                                   **bell_shape(rel)))
            for key, row in (e.get("distortion") or {}).items():
                hz, level = key.split("/")
                dist.append(dict(
                    meta, band="extra", gain_label=label, hz=float(hz),
                    level_dbfs=float(level), thd_pct=row.get("thd_pct"),
                    **{"h%d" % m: row.get("h%d" % m) for m in range(2, 9)}))

    _rows("eq_curves.csv",
          ["slug", "maker", "models", "group", "band", "target_hz",
           "actual_hz", "marking", "gain_label", "hz", "gain_db"], curves)
    _rows("eq_band_shape.csv",
          ["slug", "maker", "models", "group", "band", "target_hz",
           "actual_hz", "marking", "gain_label", "peak_db", "peak_hz", "q",
           "bandwidth_oct"], shapes)
    _rows("eq_distortion.csv",
          ["slug", "maker", "models", "group", "band", "gain_label", "hz",
           "level_dbfs", "thd_pct"] + ["h%d" % m for m in range(2, 9)], dist)
    return units


# -------------------------------------------------------------- compressors


def export_comp():
    units = load("comp")
    cal, static, timing, curves, prog, det, sc, harm, summary = (
        [], [], [], [], [], [], [], [], [])

    for slug, d in sorted(units.items()):
        meta = dict(meta_of(d), element=d.get("element"))
        for depth, e in sorted(d.get("depths", {}).items()):
            target = float(depth.split("_")[1])
            base = dict(meta, gr_target_db=target,
                        gr_measured_db=e["calibration"]["measured_gr_db"],
                        reached=e.get("reached"))
            cal.append(dict(base, control=e["calibration"]["control"],
                            value=e["calibration"]["value"],
                            refined=e["calibration"].get("refined")))

            for level, row in (e.get("static_curve") or {}).items():
                static.append(dict(base, level_dbfs=float(level),
                                   out_db=row.get("out_db"),
                                   gain_db=row.get("gain_db"),
                                   gr_db=row.get("gr_db")))

            for hz, gr in (e.get("sidechain_response_db") or {}).items():
                sc.append(dict(base, hz=float(hz), gr_db=gr))

            for hz, row in (e.get("harmonics") or {}).items():
                harm.append(dict(base, hz=float(hz),
                                 thd_pct=row.get("thd_pct"),
                                 **{"h%d" % m: row.get("h%d" % m)
                                    for m in range(2, 9)}))

            for hold, row in (e.get("programme_dependence") or {}).items():
                prog.append(dict(
                    base, hold=hold,
                    gr_at_end_of_hold_db=row.get("gr_at_end_of_hold_db"),
                    **{"release_%s_ms" % k: v
                       for k, v in (row.get("release_curve_db") or {}).items()}))

            dd = e.get("detector") or {}
            det.append(dict(base, **dd))

            marks = dict(e.get("timing") or {})
            if e.get("as_set"):
                marks["as_set"] = e["as_set"]
            for mark, row in marks.items():
                timing.append(dict(
                    base, marking=mark,
                    attack_t63_ms=row.get("attack_t63_ms"),
                    attack_t90_ms=row.get("attack_t90_ms"),
                    release_t63_ms=row.get("release_t63_ms"),
                    release_t90_ms=row.get("release_t90_ms"),
                    gr_at_end_of_hold_db=row.get("gr_at_end_of_hold_db"),
                    timing_reliable=row.get("timing_reliable"),
                    rejected_runs=row.get("rejected_runs")))
                for phase in ("attack", "release"):
                    for ms, db in (row.get("%s_curve_db" % phase) or {}).items():
                        curves.append(dict(base, marking=mark, phase=phase,
                                           time_ms=float(ms), gr_db=db))

            summary.append(_comp_summary(base, e))

    _rows("comp_calibration.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "gr_measured_db", "reached", "control", "value", "refined"], cal)
    _rows("comp_static_curve.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "level_dbfs", "out_db", "gain_db", "gr_db"], static)
    _rows("comp_timing.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "marking", "attack_t63_ms", "attack_t90_ms", "release_t63_ms",
           "release_t90_ms", "gr_at_end_of_hold_db", "timing_reliable",
           "rejected_runs"], timing)
    _rows("comp_envelope.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "marking", "phase", "time_ms", "gr_db"], curves)
    _rows("comp_programme_dependence.csv",
          sorted({k for r in prog for k in r}, key=lambda k: (
              k not in ("slug", "maker", "models", "group", "element",
                        "gr_target_db", "hold"), k)), prog)
    _rows("comp_detector.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "burst_duty", "burst_rms_below_peak_db", "gr_continuous_tone_db",
           "gr_bursts_same_peak_db", "gr_tone_at_burst_rms_db",
           "agrees_with_static"], det)
    _rows("comp_sidechain.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "hz", "gr_db"], sc)
    _rows("comp_harmonics.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "hz", "thd_pct"] + ["h%d" % m for m in range(2, 9)], harm)
    _rows("comp_summary.csv",
          ["slug", "maker", "models", "group", "element", "gr_target_db",
           "gr_measured_db", "reached", "threshold_dbfs", "ratio",
           "knee_width_db", "attack_t63_ms", "release_t63_ms",
           "timing_reliable", "programme_ratio", "detector_reads",
           "sc_50_minus_1k_db",
           "h2_1k", "h3_1k"], summary)
    return units


def _comp_summary(base, e):
    row = dict(base)
    static = e.get("static_curve") or {}
    pts = sorted((float(k), v.get("gr_db")) for k, v in static.items()
                 if v.get("gr_db") is not None)
    if pts:
        # Threshold: where reduction first passes 1 dB. Ratio: the slope
        # between the two loudest measured levels, expressed the way a
        # front panel would. Knee: how many dB of input it takes to get
        # from a tenth of that slope to nine tenths of it.
        row["threshold_dbfs"] = interp_level_at(pts, 1.0)
        (l0, g0), (l1, g1) = pts[-2], pts[-1]
        slope = (g1 - g0) / (l1 - l0) if l1 != l0 else 0.0
        row["ratio"] = round(1.0 / max(1e-6, 1.0 - slope), 2) if slope < 1 else None
        if slope > 0:
            lo = interp_level_at([(l, g) for l, g in pts], 0.1 * pts[-1][1])
            hi = interp_level_at([(l, g) for l, g in pts], 0.9 * pts[-1][1])
            if lo is not None and hi is not None:
                row["knee_width_db"] = round(hi - lo, 2)

    marks = e.get("timing") or {}
    as_set = e.get("as_set") or {}
    row["attack_t63_ms"] = as_set.get("attack_t63_ms")
    row["release_t63_ms"] = as_set.get("release_t63_ms")
    row["timing_reliable"] = as_set.get("timing_reliable")

    prog = e.get("programme_dependence") or {}
    short = (prog.get("hold_0.03s") or {}).get("release_curve_db", {}).get("500")
    long = (prog.get("hold_3s") or {}).get("release_curve_db", {}).get("500")
    if short and long and short > 0.01:
        # How much more reduction is still in force half a second after a
        # long note than after a short one. The first pass found this splits
        # exactly along the gain element: opto and FET have it, VCA does not.
        row["programme_ratio"] = round(long / short, 2)

    det = e.get("detector") or {}
    tone = det.get("gr_continuous_tone_db")
    bursts = det.get("gr_bursts_same_peak_db")
    quiet = det.get("gr_tone_at_burst_rms_db")
    if None not in (tone, bursts, quiet):
        # A peak detector treats bursts like the loud tone; an RMS detector
        # treats them like the quiet one. Whichever it is nearer, it is.
        row["detector_reads"] = ("peak" if abs(bursts - tone)
                                 < abs(bursts - quiet) else "rms")

    sc = e.get("sidechain_response_db") or {}
    if "50" in sc and "1000" in sc:
        row["sc_50_minus_1k_db"] = round(sc["50"] - sc["1000"], 3)

    h = (e.get("harmonics") or {}).get("1000") or {}
    row["h2_1k"], row["h3_1k"] = h.get("h2"), h.get("h3")
    return row


def export_noise():
    """Self-noise with each unit's noise model in and out, as a pair."""
    units = load("noise")
    bands, summary = [], []
    for slug, d in sorted(units.items()):
        meta = meta_of(d)
        states = d.get("states", {})
        # Paired per condition, not just on silence. Several of these models
        # emit nothing at all until signal is passing -- Waves NLS Channel is
        # silent on silence at every setting and puts a floor 17 dB higher
        # under a tone once its Drive is up -- so a "noise added" figure
        # taken only on silence reports zero for exactly the units that have
        # a noise model worth knowing about.
        def added(cond):
            on = ((states.get("on") or {}).get(cond) or {}).get("rms_dbfs")
            off = ((states.get("off") or {}).get(cond) or {}).get("rms_dbfs")
            return round(on - off, 2) if None not in (on, off) else None

        for state, e in sorted(states.items()):
            for cond, r in sorted(e.items()):
                if cond == "applied" or not r:
                    continue
                for hz, db in (r.get("third_octave_dbfs") or {}).items():
                    bands.append(dict(meta, state=state, condition=cond,
                                      hz=float(hz), level_dbfs=db))
                summary.append(dict(
                    meta, state=state, condition=cond,
                    rms_dbfs=r.get("rms_dbfs"),
                    peak_dbfs=r.get("peak_dbfs"),
                    a_weighted_dbfs=r.get("a_weighted_dbfs"),
                    noise_added_db=added(cond),
                    controls=" ".join("%s=%s" % kv for kv in
                                      sorted((d.get("controls_found")
                                              or {}).items())),
                    loudest_tone_hz=(r.get("tones") or [{}])[0].get("hz"),
                    loudest_tone_dbfs=(r.get("tones") or [{}])[0].get("dbfs")))
    _rows("noise_bands.csv",
          ["slug", "maker", "models", "family", "group", "state", "condition",
           "hz", "level_dbfs"], bands)
    _rows("noise_summary.csv",
          ["slug", "maker", "models", "family", "group", "state", "condition",
           "rms_dbfs", "peak_dbfs", "a_weighted_dbfs", "noise_added_db",
           "controls", "loudest_tone_hz", "loudest_tone_dbfs"], summary)
    return units


def export_units():
    """The index: every unit, what it models, and which suites measured it.

    `latency_samples` and the as-shipped readings come from the survey rather
    than from a measurement run, so they are there even for a unit whose
    sweep failed -- which is what makes this the table to join the others to.
    """
    import roster
    rows = []
    for slug, u in sorted(roster.UNITS.items()):
        measured = [fam for fam in ("colour", "eq", "comp", "noise")
                    if os.path.exists("out/chart/%s/%s.json" % (fam, slug))]
        row = dict(slug=slug, maker=u["maker"], models=u["models"],
                   family=u["family"], group=u["group"],
                   plugin=u["spec"][1] or os.path.basename(u["spec"][0]),
                   measured_in=" ".join(measured), note=u["note"])
        path = "out/survey/%s.json" % slug
        if os.path.exists(path):
            d = json.load(open(path))
            row["survey_ok"] = "error" not in d and not d.get("silent")
            row["latency_samples"] = d.get("latency_samples")
            at = d.get("at_-18_dbfs") or {}
            row["default_gain_db"] = at.get("gain_db")
            row["default_thd_pct"] = at.get("thd_pct")
        # The survey loads a unit and measures it as it arrives, before any
        # of `voicings.py` has been applied -- which for the Airwindows units
        # means whatever state the previous processor left behind. Where a
        # measurement run exists it saw the unit configured, so its latency
        # is the one to quote.
        for fam in measured:
            d = json.load(open("out/chart/%s/%s.json" % (fam, slug)))
            if d.get("latency_samples") is not None:
                row["latency_samples"] = d["latency_samples"]
                break
        rows.append(row)
    _rows("units.csv",
          ["slug", "maker", "models", "family", "group", "plugin",
           "measured_in", "survey_ok", "latency_samples", "default_gain_db",
           "default_thd_pct", "note"], rows)


if __name__ == "__main__":
    which = sys.argv[1:] or ["units", "colour", "eq", "comp", "noise"]
    if "units" in which:
        export_units()
    if "colour" in which:
        export_colour()
    if "eq" in which:
        export_eq()
    if "comp" in which:
        export_comp()
    if "noise" in which:
        export_noise()
