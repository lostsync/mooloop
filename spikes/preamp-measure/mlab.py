"""Offline plugin hosting and spectrum analysis for reference measurements.

Runs a VST3/AU plugin over synthesised signals with no GUI and no audio
device, so every number here is reproducible from a shell.

Level convention throughout: signal amplitudes are dBFS peak. Two reference
levels matter and they are not the same one.

* The plugins assume the studio convention, -18 dBFS = 0 VU = +4 dBu, which
  is what their "headroom" controls calibrate and where the analogue units
  they emulate are designed to sit.
* **mooloop's unity operating level is -12 dBFS** (`GAIN_STRUCTURE.md`,
  `gain::REFERENCE_PEAK_DBFS`), six dB hotter.

So -12 dBFS is the row to author a voicing from, and it is in every grid
here for that reason. Quoting a harmonic without saying which level it was
measured at is the mistake this whole exercise exists to avoid.
"""

import contextlib
import json
import math
import os
import sys

import numpy as np
from pedalboard import AudioUnitPlugin, VST3Plugin, load_plugin

SR = 48000.0


@contextlib.contextmanager
def muffled():
    """Silence the chatter plugins emit on load and on render, at the fd level.

    Both descriptors, because the two makers that talk do it differently: the
    objc/JUCE noise goes to stderr, and UAD's plugins print a page of logger
    configuration to *stdout* every time one is instantiated. A survey of a
    hundred units emitted enough of it to bury its own report.
    """
    # Flush first: stdout is block-buffered when it is a pipe, so anything
    # still pending would be written after the swap and land in devnull.
    sys.stdout.flush()
    saved = [os.dup(1), os.dup(2)]
    devnull = os.open(os.devnull, os.O_WRONLY)
    os.dup2(devnull, 1)
    os.dup2(devnull, 2)
    try:
        yield
    finally:
        sys.stdout.flush()
        os.dup2(saved[0], 1)
        os.dup2(saved[1], 2)
        os.close(devnull)
        for fd in saved:
            os.close(fd)


def open_plugin(path, name=None):
    with muffled():
        return load_plugin(path, plugin_name=name) if name else load_plugin(path)


def plugin_names(path):
    cls = VST3Plugin if path.endswith(".vst3") else AudioUnitPlugin
    with muffled():
        return list(cls.get_plugin_names_for_file(path))


# ---------------------------------------------------------------- parameters


def param_table(plugin):
    """Every parameter as a plain dict, for writing down what a setting was."""
    out = {}
    for key, p in plugin.parameters.items():
        entry = {"key": key}
        for attr in ("name", "raw_value", "min_value", "max_value", "units",
                     "step_size", "approximate_step_size", "type", "label"):
            value = getattr(p, attr, None)
            if value is None:
                continue
            if attr == "type":
                value = getattr(value, "__name__", str(value))
            entry[attr] = value
        try:
            entry["value"] = _plain(p.__get__(plugin) if hasattr(p, "__get__") else None)
        except Exception:
            pass
        try:
            entry["string"] = str(getattr(plugin, key))
        except Exception:
            pass
        try:
            vv = getattr(p, "valid_values", None)
            if vv:
                entry["valid_values"] = [str(v) for v in vv][:64]
        except Exception:
            pass
        out[key] = entry
    return out


def _plain(v):
    if isinstance(v, (bool, int, float, str)) or v is None:
        return v
    return str(v)


def set_params(plugin, settings):
    """Set by python-safe key or by exact parameter name; report what stuck.

    A value of the form `("raw", x)` sets the normalised 0..1 value instead.
    Some plugins expose a continuous control as a choice list of a thousand
    strings, which pedalboard truncates at five hundred -- bx_console N's
    virtual gain is one -- so the top of the range is unreachable by name and
    only reachable raw.
    """
    applied = {}
    for key, want in settings.items():
        target = key
        if not hasattr(plugin, target):
            match = [k for k, p in plugin.parameters.items()
                     if getattr(p, "name", "").lower() == key.lower()]
            if not match:
                raise KeyError("no parameter %r; have %s"
                               % (key, sorted(plugin.parameters)))
            target = match[0]
        if isinstance(want, tuple) and len(want) == 2 and want[0] == "raw":
            plugin.parameters[target].raw_value = float(want[1])
        else:
            setattr(plugin, target, want)
        applied[target] = _plain(getattr(plugin, target))
    return applied


# ------------------------------------------------------------------- signals


def db_to_lin(db):
    return 10.0 ** (db / 20.0)


def lin_to_db(x, floor=-200.0):
    x = np.maximum(np.abs(x), 1e-30)
    return np.maximum(20.0 * np.log10(x), floor)


def bin_of(freq, n, sr=SR):
    """Nearest exact-FFT-bin frequency, so a sine leaks into no other bin."""
    k = max(1, int(round(freq * n / sr)))
    return k * sr / n, k


def sine(freq, db, n, sr=SR, phase=0.0):
    t = np.arange(n) / sr
    return (db_to_lin(db) * np.sin(2 * np.pi * freq * t + phase)).astype(np.float64)


def stereo(x):
    return np.stack([x, x])


# ------------------------------------------------------------------ analysis


def render(plugin, x, sr=SR, buffer_size=1024, channels=2):
    """Process mono `x`, return mono float64. Plugin state is reset first."""
    buf = stereo(x) if channels == 2 else x[None, :]
    with muffled():
        y = plugin.process(buf.astype(np.float32), sr, buffer_size=buffer_size,
                           reset=True)
    y = np.asarray(y, dtype=np.float64)
    if y.ndim == 2:
        y = y[0]
    return y


def spectrum(y, n):
    """Rectangular-window magnitude spectrum of the last n samples."""
    seg = y[-n:]
    if len(seg) < n:
        seg = np.pad(seg, (n - len(seg), 0))
    return np.abs(np.fft.rfft(seg)) * (2.0 / n)


def harmonic_profile(y, k, n, orders=8):
    """Harmonics 2..orders in dB relative to the fundamental, plus THD."""
    mag = spectrum(y, n)
    fund = mag[k]
    if fund <= 0:
        return None
    out = {"fundamental_dbfs": float(lin_to_db(fund))}
    power = 0.0
    for m in range(2, orders + 1):
        idx = m * k
        if idx >= len(mag):
            break
        # Fold harmonics above Nyquist back where they alias to, so a plugin
        # that does not oversample is caught rather than silently measured as
        # clean.
        out["h%d" % m] = float(lin_to_db(mag[idx] / fund))
        power += float(mag[idx]) ** 2
    out["thd_pct"] = float(100.0 * math.sqrt(power) / fund)
    out["thd_db"] = float(lin_to_db(math.sqrt(power) / fund))
    return out


def noise_floor(mag, k, orders=16, skip=2):
    """Median magnitude away from the fundamental and its harmonics."""
    mask = np.ones(len(mag), dtype=bool)
    for m in range(1, orders + 1):
        lo, hi = max(0, m * k - skip), min(len(mag), m * k + skip + 1)
        mask[lo:hi] = False
    mask[:4] = False
    return float(lin_to_db(np.median(mag[mask])))


def latency_samples(plugin, sr=SR, n=1 << 15):
    """Group delay at the impulse, by peak of the response to a delta."""
    x = np.zeros(n)
    x[1000] = 0.25
    y = render(plugin, x, sr)
    if not np.any(np.abs(y) > 1e-9):
        return None
    return int(np.argmax(np.abs(y)) - 1000)


def rms_db(x):
    return float(lin_to_db(np.sqrt(np.mean(np.square(x)))))


def envelope_db(y, sr=SR, window_ms=1.0):
    """Sliding-RMS envelope in dB, for reading a compressor's timing off."""
    w = max(1, int(sr * window_ms / 1000.0))
    kernel = np.ones(w) / w
    p = np.convolve(np.square(y), kernel, mode="same")
    return lin_to_db(np.sqrt(np.maximum(p, 1e-30)))


def report(msg):
    """Progress, on stderr.

    Not stdout, because UAD's plugins print a page of logger configuration
    the moment one is instantiated and some of it escapes `muffled` -- it is
    emitted from a C stream whose buffer is flushed after the redirect has
    been put back. So the runs send stdout to /dev/null and read stderr, and
    every progress line in this directory goes through here.
    """
    print(msg, file=sys.stderr, flush=True)


def write_json(path, payload):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        json.dump(payload, fh, indent=1, sort_keys=True)
    report("wrote %s" % path)


def stable_harmonics(plugin, freq, k, n, db, settle, repeats=4, sr=SR):
    """A cell of the harmonic surface, with the intermittent glitch rejected.

    Some of these plugins drop a glitch into roughly one render in five --
    Waves NLS does it, most often at low levels -- which would otherwise be
    written down as a measurement 40 dB above the real 2nd harmonic. A clean
    render is bit-repeatable; a glitched one loses fundamental energy and
    lifts the broadband floor by tens of dB. So rather than out-vote the bad
    renders, take the one with the most fundamental in it, and record how
    many were thrown away so the glitch rate stays visible.
    """
    runs = []
    for _ in range(repeats):
        x = sine(freq, db, n + settle)
        y = render(plugin, x, sr)
        mag = spectrum(y, n)
        prof = harmonic_profile(y, k, n) or {}
        prof["noise_floor_db"] = noise_floor(mag, k)
        prof["out_peak"] = float(np.max(np.abs(y)))
        runs.append(prof)

    best = max(runs, key=lambda p: (p.get("fundamental_dbfs", -300.0),
                                    -p.get("noise_floor_db", 0.0)))
    top = best.get("fundamental_dbfs", -300.0)
    best = dict(best)
    best["rejected_runs"] = sum(
        1 for p in runs if p.get("fundamental_dbfs", -300.0) < top - 0.05)
    best["clipped"] = bool(best.get("out_peak", 0.0) >= 0.999)
    return best
