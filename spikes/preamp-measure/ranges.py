"""Exact ranges and choice lists for named parameters, without the flood.

Some choice lists here run to a thousand entries; printing one in full once
cost more than the measurement it was setting up.
"""

import sys

import mlab
from candidates import CANDIDATES


def show(slug, keys):
    path, name, _, note = CANDIDATES[slug]
    p = mlab.open_plugin(path, name)
    print("=====", slug, "--", note)
    for k in keys:
        if not hasattr(p, k):
            print("   %-16s MISSING" % k)
            continue
        par = p.parameters[k]
        vv = list(getattr(par, "valid_values", None) or [])
        if len(vv) > 14:
            vv = [vv[0], vv[1], "...", vv[-2], vv[-1]]
        print("   %-16s cur=%-12r range=(%s, %s) valid=%s"
              % (k, getattr(p, k), getattr(par, "min_value", None),
                 getattr(par, "max_value", None), vv or None))


if __name__ == "__main__":
    show(sys.argv[1], sys.argv[2:])
