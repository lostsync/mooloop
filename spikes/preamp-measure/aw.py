"""Selecting one Airwindows processor out of Airwindows Consolidated.

Airwindows ships as four hundred separate VST2 plugins, and pedalboard hosts
VST3 and AU only -- so none of it is reachable the way every other unit here
is. The consolidated build *is* a VST3, but it picks which of the four
hundred it runs from a dropdown rather than from a parameter, so a headless
host has no way to ask.

It does have one, indirectly. The plugin's VST3 preset chunk is plain XML
with the processor's name in it:

    <awconsolidated streamingVersion="8524" currentProcessorName="Galactic"
                    awp_0="0.5" ... />

So a preset can be rewritten with a different name and handed back through
`preset_data`, and the plugin loads that processor and republishes its
parameters. `select()` does exactly that and then *checks*, because an
unrecognised name is silently ignored and leaves the previous processor in
place -- which would measure the wrong plugin under the right label.

Getting the bundle onto the machine, once, without installing anything:

    hdiutil attach -readonly -nobrowse airwindows-consolidated-*.dmg
    pkgutil --expand-full /Volumes/airwindows-*/airwindows-*.pkg /tmp/aw
    cp -R "/tmp/aw/airwindows-consolidated_VST3.pkg/Payload/Airwindows Consolidated.vst3" \
          ~/mooloop-measure/plugins/
    xattr -dr com.apple.quarantine ~/mooloop-measure/plugins/Airwindows*.vst3

Nothing lands in /Library; the bundle lives beside the scripts.
"""

import os
import re
import struct

import mlab

PATH = os.path.expanduser("~/mooloop-measure/plugins/Airwindows Consolidated.vst3")

_NAME = re.compile(rb'currentProcessorName="([^"]*)"')

_plugin = None
_base = None


def _repack(preset, name):
    """Same preset with a different processor name, offsets fixed up.

    A VST3 preset is a 48-byte header carrying the offset of a chunk list, a
    run of chunks, then the list. Changing the XML changes the length of the
    first chunk, so the header's offset and both of the list's entries have
    to move with it or the plugin reads past the end of its own state.
    """
    list_offset = struct.unpack("<q", preset[40:48])[0]
    head, body, tail = preset[:48], preset[48:list_offset], preset[list_offset:]
    assert body[:4] == b"VC2!", "not a JUCE VST3 state chunk"
    xml_len = struct.unpack("<i", body[4:8])[0]
    xml, rest = body[8:8 + xml_len], body[8 + xml_len:]

    new_xml = _NAME.sub(('currentProcessorName="%s"' % name).encode(), xml)
    body = b"VC2!" + struct.pack("<i", len(new_xml)) + new_xml + rest
    new_offset = 48 + len(body)
    out = head[:40] + struct.pack("<q", new_offset) + head[48:] + body + tail

    count = struct.unpack("<i", out[new_offset + 4:new_offset + 8])[0]
    at = new_offset + 8
    for _ in range(count):
        cid = out[at:at + 4]
        off, size = struct.unpack("<qq", out[at + 4:at + 20])
        if cid == b"Comp":
            off, size = 48, len(body)
        elif cid == b"Cont":
            off = new_offset
        out = out[:at + 4] + struct.pack("<qq", off, size) + out[at + 20:]
        at += 20
    return out


def open_shell():
    """The one instance, kept open: loading this bundle costs a second."""
    global _plugin, _base
    if _plugin is None:
        _plugin = mlab.open_plugin(PATH)
        _base = _plugin.preset_data
    return _plugin


def current_name(plugin=None):
    m = _NAME.search((plugin or open_shell()).preset_data)
    return m.group(1).decode() if m else None


def select(name):
    """Load one Airwindows processor by name, or raise.

    The check is the point. A name the build does not have is ignored rather
    than refused, and the previous processor keeps running -- which is a
    measurement of the wrong plugin filed under the right slug.
    """
    plugin = open_shell()
    plugin.preset_data = _repack(_base, name)
    got = current_name(plugin)
    if got != name:
        raise KeyError("Airwindows Consolidated has no %r (still on %r)"
                       % (name, got))
    return plugin


def available(names):
    """Which of `names` this build actually has, checked one at a time."""
    ok = []
    for name in names:
        try:
            select(name)
            ok.append(name)
        except KeyError:
            pass
    return ok


if __name__ == "__main__":
    import sys
    for slug in sys.argv[1:]:
        try:
            p = select(slug)
            print("%-24s %s" % (slug, sorted(p.parameters)))
        except KeyError as exc:
            print("%-24s MISSING (%s)" % (slug, exc))
