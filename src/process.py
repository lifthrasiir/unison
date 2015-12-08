# coding: utf-8

import re
import unicodedata
from collections import namedtuple

# pixels
PX_SUBPIXEL     = 0x1f # subpixel mask
PX_FULL         = 0x20

M = PX_SUBPIXEL
PX_EMPTY        =  0   # .
PX_ALMOSTFULL   =  0^M # @
PX_HALF1        =  1   # |\  b
PX_HALF2        =  1^M #  \| 9
PX_HALF3        =  2   # |/  P
PX_HALF4        =  2^M #  /| d
PX_QUAD1        =  3   # |>  |)
PX_QUAD2        =  4   #  v   u
PX_QUAD3        =  5   #  <|  (|
PX_QUAD4        =  6   #  ^   n
PX_INVQUAD1     =  3^M #  >|  )|
PX_INVQUAD2     =  4^M # |v| |u|
PX_INVQUAD3     =  5^M # |<  |(
PX_INVQUAD4     =  6^M # |^| |n|
PX_SLANT1H      =  7   # same to PX_HALF* but shifted and scaled horizontally by 1/2
PX_SLANT2H      =  8   #   ....               ....
PX_SLANT3H      =  9   #   : /| = PX_HALF4    .  | = PX_SLANT4H
PX_SLANT4H      = 10   #   :/_|               ../|
PX_SLANT1V      = 11   # same to PX_HALF* but shifted and scaled vertically by 1/2
PX_SLANT2V      = 12   #   ....               ....
PX_SLANT3V      = 13   #   : /| = PX_HALF4    .  : = PX_SLANT4V
PX_SLANT4V      = 14   #   :/_|               ._/|
PX_HALFSLANT1H  =  8^M # same to PX_HALF* but shifted and scaled horizontally by 3/2
PX_HALFSLANT2H  =  7^M #   ....               ....
PX_HALFSLANT3H  = 10^M #   : /| = PX_HALF4    . || = PX_HALFSLANT4H
PX_HALFSLANT4H  =  9^M #   :/_|               ./_|
PX_HALFSLANT1V  = 12^M # same to PX_HALF* but shifted and scaled vertically by 3/2
PX_HALFSLANT2V  = 11^M #   ....               ....
PX_HALFSLANT3V  = 14^M #   : /| = PX_HALF4    ._/| = PX_HALFSLANT4V
PX_HALFSLANT4V  = 13^M #   :/_|               |__|
PX_DOT          = 15   # *

ADJACENCY_MAP = {
    #    a   b
    #   +--+--+
    # h |     | c
    #   +     +   <---+
    # g |     | d     |
    #   +--+--+  (adjacency) (the line segment corresponding to the gap)
    #    f   e     abcdefgh; [(start x, y, end x, y), ...]
    PX_EMPTY:   (0b00000000, []),
    PX_HALF1:   (0b00001111, [(0, 0, 1, 1)]),
    PX_HALF3:   (0b11000011, [(0, 1, 1, 0)]),
    PX_QUAD1:   (0b00000011, [(0, 0, 0.5, 0.5), (0.5, 0.5, 0, 1)]),
    PX_QUAD2:   (0b11000000, [(0, 0, 0.5, 0.5), (0.5, 0.5, 1, 0)]),
    PX_QUAD3:   (0b00110000, [(1, 0, 0.5, 0.5), (0.5, 0.5, 1, 1)]),
    PX_QUAD4:   (0b00001100, [(0, 1, 0.5, 0.5), (0.5, 0.5, 1, 1)]),
    PX_SLANT1H: (0b00000111, [(0, 0, 0.5, 1)]),
    PX_SLANT2H: (0b01110000, [(0.5, 0, 1, 1)]),
    PX_SLANT3H: (0b10000011, [(0, 1, 0.5, 0)]),
    PX_SLANT4H: (0b00111000, [(0.5, 1, 1, 0)]),
    PX_SLANT1V: (0b00001110, [(0, 0.5, 1, 1)]),
    PX_SLANT2V: (0b11100000, [(0, 0, 1, 0.5)]),
    PX_SLANT3V: (0b11000001, [(0, 0.5, 1, 0)]),
    PX_SLANT4V: (0b00011100, [(0, 1, 1, 0.5)]),
    PX_DOT:     (0b00000000, [(0, 0.5, 0.5, 0), (0.5, 0, 1, 0.5),
                              (1, 0.5, 0.5, 1), (0.5, 1, 0, 0.5)]),
}
ADJACENCY = []
for k in xrange(0x20):
    try: ADJACENCY.append(ADJACENCY_MAP[k])
    except KeyError:
        adj, segments = ADJACENCY_MAP[k ^ PX_SUBPIXEL]
        ADJACENCY.append((adj ^ 0b11111111, segments))
ADJACENCY.append(ADJACENCY[PX_ALMOSTFULL]) # used as the entry for PX_FULL

LEFTMOST = [0] * len(ADJACENCY)
LEFTMOST[PX_EMPTY] = None
LEFTMOST[PX_QUAD3] = LEFTMOST[PX_SLANT2H] = LEFTMOST[PX_SLANT4H] = 0.5


Delta = namedtuple('Delta', 'value')

# data = None | list of pixel codes | subglyph name string
# top/left can be a Delta value instead of the actual value; will be resolved later.
Subglyph = namedtuple('Subglyph', 'top left height width stride data')

# height/width should be the intrinsic height/width, and may be filled by resolve_glyphs.
# preferred_top/left becomes the actual offset when the glyph is used at the top level.
Glyph = namedtuple('Glyph', 'height width preferred_top preferred_left subglyphs')

class ParseError(ValueError): pass

def escape(s):
    return s.encode('utf-8').replace('&','&amp;').replace('"', '&quot;')

def glyph_name(i):
    if isinstance(i, int):
        try: name = u' ' + unicodedata.name(unichr(i))
        except ValueError: name = u''
        return u'U+%04X%s (%c)' % (i, name, i)
    else:
        return repr(i.encode())

def ensure_sentinels(g, always_copy=False):
    if not isinstance(g.data, list): return g
    if g.stride > g.width and len(g.data) >= g.stride * (g.height + 1):
        if always_copy: g = g._replace(data=g.data[:])
        return g

    stride = g.width + 1
    data = []
    for r in xrange(g.height):
        data.extend(g.data[r*g.stride:r*g.stride+g.width])
        data.append(PX_EMPTY)
    data.extend([PX_EMPTY] * stride)
    return g._replace(stride=stride, data=data)

def merge_subglyphs_sans_subpixel(height, width, subglyphs):
    stride = width + 1
    data = [PX_EMPTY] * ((height + 1) * stride)
    for g in subglyphs:
        gtop, gleft, gheight, gwidth, gstride, gdata = g
        assert gleft is not None and not isinstance(gleft, Delta)
        assert gtop is not None and not isinstance(gtop, Delta)
        assert len(gdata) == gwidth * gheight
        for r in xrange(gheight):
            for c in xrange(gwidth):
                data[(gtop+r)*stride+(gleft+c)] |= gdata[r*gstride+c]
    return Subglyph(top=0, left=0, height=height, width=width, stride=stride, data=data)

def signed_area(path):
    x0, y0 = path[-1]
    area = 0
    for i, (x, y) in enumerate(path):
        area += x0 * y - x * y0
        x0 = x
        y0 = y
    return area

# 3-----2   2-----3
#       |   |         1--2--3
#       1   1
# ccw > 0   ccw < 0   ccw = 0
def ccw(x1, y1, x2, y2, x3, y3):
    return (x2 - x1) * (y3 - y1) - (y2 - y1) * (x3 - x1)

# http://geomalgorithms.com/a03-_inclusion.html
def winding_number(x, y, path):
    xx0, yy0 = path[-1]
    wn = 0
    for i, (xx, yy) in enumerate(path):
        if yy0 <= y:
            if yy > y and ccw(xx0, yy0, xx, yy, x, y) > 0: wn += 1
        else:
            if yy <= y and ccw(xx0, yy0, xx, yy, x, y) < 0: wn -= 1
        xx0 = xx
        yy0 = yy
    return wn

def track_contour(height, width, stride0, data0, mask):
    # add sentinels around the pixels (would be progressively removed during traversal)
    stride = width + 1
    data = []
    for r in xrange(height):
        data.extend(data0[r*stride0:r*stride0+width])
        data.append(PX_EMPTY)
    data.extend([PX_EMPTY] * stride)

    paths = []
    visited = set()
    for i0 in xrange(0, height * stride, stride):
        for i in xrange(i0, i0 + width):
            # find the first non-empty pixel unvisited
            if data[i] == PX_EMPTY: continue
            if i in visited: continue

            unsure = set([i])
            segs = []
            while unsure:
                i = unsure.pop()
                visited.add(i)

                pixel,  gapsegs = ADJACENCY[data[i]        & mask]
                top,    _       = ADJACENCY[data[i-stride] & mask]
                bottom, _       = ADJACENCY[data[i+stride] & mask]
                left,   _       = ADJACENCY[data[i-1]      & mask]
                right,  _       = ADJACENCY[data[i+1]      & mask]

                connected = (
                    (pixel & (top    << 5) & 0b10000000) |
                    (pixel & (top    << 3) & 0b01000000) |
                    (pixel & (right  << 5) & 0b00100000) |
                    (pixel & (right  << 3) & 0b00010000) |
                    (pixel & (bottom >> 3) & 0b00001000) |
                    (pixel & (bottom >> 5) & 0b00000100) |
                    (pixel & (left   >> 3) & 0b00000010) |
                    (pixel & (left   >> 5) & 0b00000001)
                )

                if (connected & 0b11000000) and (i-stride) not in visited: unsure.add(i-stride)
                if (connected & 0b00110000) and (i+1)      not in visited: unsure.add(i+1)
                if (connected & 0b00001100) and (i+stride) not in visited: unsure.add(i+stride)
                if (connected & 0b00000011) and (i-1)      not in visited: unsure.add(i-1)

                disconnected = connected ^ 0b11111111
                if disconnected:
                    y, x = divmod(i, stride)

                    linesegs = pixel & disconnected
                    if linesegs & 0b11000000: segs.append((x, y, x + 1, y))
                    else:
                        if linesegs & 0b10000000: segs.append((x, y, x + 0.5, y))
                        if linesegs & 0b01000000: segs.append((x + 0.5, y, x + 1, y))
                    if linesegs & 0b00110000: segs.append((x + 1, y, x + 1, y + 1))
                    else:
                        if linesegs & 0b00100000: segs.append((x + 1, y, x + 1, y + 0.5))
                        if linesegs & 0b00010000: segs.append((x + 1, y + 0.5, x + 1, y + 1))
                    if linesegs & 0b00001100: segs.append((x + 1, y + 1, x, y + 1))
                    else:
                        if linesegs & 0b00001000: segs.append((x + 1, y + 1, x + 0.5, y + 1))
                        if linesegs & 0b00000100: segs.append((x + 0.5, y + 1, x, y + 1))
                    if linesegs & 0b00000011: segs.append((x, y + 1, x, y))
                    else:
                        if linesegs & 0b00000010: segs.append((x, y + 1, x, y + 0.5))
                        if linesegs & 0b00000001: segs.append((x, y + 0.5, x, y))

                    if ~pixel & disconnected:
                        segs.extend((x + x1, y + y1, x + x2, y + y2) for x1, y1, x2, y2 in gapsegs)

            pxtosegs = {}
            for x1, y1, x2, y2 in segs:
                pxtosegs.setdefault((x1, y1), []).append((x2, y2))
                pxtosegs.setdefault((x2, y2), []).append((x1, y1))

            while pxtosegs:
                (x0, y0), v = pxtosegs.popitem()
                assert len(v) >= 2
                x, y = v.pop()
                pxtosegs[x0, y0] = v

                xorg = x0
                yorg = y0
                dx = x0 - x
                dy = y0 - y
                path = [(xorg, yorg)]
                indices = {(xorg, yorg): 0}
                while True:
                    nx = pxtosegs.pop((x, y))
                    nx.remove((x0, y0))

                    # check for the cycle...
                    k = indices.get((x, y))
                    if k is not None:
                        # then extract that cycle.
                        # should re-add the first point if it were elided previously.
                        extracted = path[k:]
                        del path[k:]
                        if extracted[0] != (x, y): extracted.insert(0, (x, y))
                        paths.append(extracted)

                        # if nothing remains, we've reached the initial point.
                        if not path:
                            if nx: pxtosegs[x, y] = nx
                            break

                        for k, v in indices.items():
                            if v >= len(path): del k

                        # simulate the state right after (x, y) was originally added
                        px, py = path[-1]
                        dx = x - px
                        dy = y - py

                    xx, yy = nx.pop(0)
                    if nx: pxtosegs[x, y] = nx

                    # flush the previous segment if the current segment has a different slope
                    indices[x, y] = len(path)
                    if dx * (y - yy) != dy * (x - xx):
                        path.append((x, y))
                        dx = x - xx
                        dy = y - yy

                    x0 = x
                    y0 = y
                    x = xx
                    y = yy

    # fix the winding directions of resulting contours
    for path in paths:
        # pick any point in the path, and sum the winding numbers of that point in other paths.
        # the point should be not in other paths being compared.
        wn = 0
        for p in paths:
            if p is path: continue # "any" point would always coincide
            for x, y in path:
                if (x, y) not in p: break
            else: assert False
            wn += winding_number(x, y, p)

        # if a sign of the signed area mismatches with the winding number, reverse.
        # (the signed area only works with the simple polygon, which is the case)
        a = signed_area(path)
        if ((wn & 1) == 1) ^ (a < 0): path.reverse()

    return paths

class Font(object):
    def __init__(self, fp=None):
        # +-----+               ^
        # |     |               |
        # |_____|  ^            |
        # | @@@ |  |            |
        # |@   @|  | ascent     |
        # |@@@@@|  |            | height
        # |@   @|  v            |
        # ============ baseline |
        # |     |  ^            |
        # |     |  | descent    |
        # +-----+  v            v
        self.height = 16
        self.ascent = 16
        self.descent = 0 # XXX probably needs the downward translation in the glyph itself

        self.glyphs = {} # name: Glyph

        if fp: self.read(fp)

    def read(self, fp):
        GLYPH_NAME_PATTERN = re.compile(ur'''^(?:
            # unicode range
            u\+(?P<start>[0-9a-f]{4,8})(?:\.\.(?P<end>[0-9a-f]{4,8})(?:/(?P<step>[0-9a-f]+))?)? |
            # name list or character list
            (?P<names>(?:.|[0-9a-z\-_.]+)(?:\*\d+)?(?:\|(?:.|[0-9a-z\-_.]+)(?:\*\d+)?)*) |
            # name list with prefix and suffix
            (?P<prefix>[0-9a-z\-_.]*)\(
                (?P<nameparts>[0-9a-z\-_.]+(?:\*\d+)?(?:\|[0-9a-z\-_.]+(?:\*\d+)?)*)
            \)(?P<suffix>[0-9a-z\-_.]*)
        )$''', re.I | re.X)
        def parse_glyph_name(s):
            m = GLYPH_NAME_PATTERN.match(s)
            if not m: raise ParseError(u'invalid glyph name/index %r' % s)

            if m.group('start'):
                start = int(m.group('start'), 16)
                end = m.group('end'); end = int(end, 16) if end is not None else None
                step = m.group('step'); step = int(step, 16) if step is not None else 1
                if end is not None:
                    return xrange(start, end + 1, step)
                else:
                    assert step == 1
                    return start

            prefix = m.group('prefix') or u''
            suffix = m.group('suffix') or u''
            parts = m.group('names') or m.group('nameparts')
            ss = []
            for part in parts.split('|'):
                n, _, rep = part.partition('*') if part != u'*' else (u'*', None, None)
                n = prefix + n + suffix
                if len(n) == 1:
                    n = ord(n)
                else:
                    n = n.lower()
                ss.extend([n] * int(rep or 1))
            if any(c in u'*|()' for c in s):
                return ss
            else:
                return ss[0]

        def parse_subglyph_spec(s):
            if len(s) >= 3:
                name, _, placeholder = s.partition('@')
                if name:
                    if placeholder and all(c in '^v<>' for c in placeholder):
                        # displacement spec. e.g. ced@vv moves a cedilla one pixel down
                        roff = placeholder.count('v') - placeholder.count('^')
                        coff = placeholder.count('>') - placeholder.count('<')
                        return parse_glyph_name(name), None, roff, coff
                    elif len(placeholder) == 1:
                        return parse_glyph_name(name), placeholder, None, None
            return parse_glyph_name(s), None, None, None

        def parse_pixels(lines, bbox):
            width = len(lines[0])
            height = len(lines)
            pixels = []
            FILLED = '@b9Pd(u)n'
            for rr, line in enumerate(lines):
                assert len(line) == width
                if not (bbox[0] <= rr <= bbox[2]): continue
                for cc, px in enumerate(line):
                    if not (bbox[1] <= cc <= bbox[3]): continue
                    v = None
                    t = lines[rr-1][cc]; b = lines[rr+1][cc]
                    l = lines[rr][cc-1]; r = lines[rr][cc+1]
                    if px == '@':
                        v = PX_ALMOSTFULL
                    elif px == '*':
                        v = PX_DOT
                    elif px in '.!+':
                        v = PX_EMPTY
                    elif px == 'b':
                        v = ((None           if r=='\\' else PX_HALFSLANT1H) if t=='\\' else
                             (PX_HALFSLANT1V if r=='\\' else PX_HALF1))
                    elif px == '9':
                        v = ((None           if l=='\\' else PX_HALFSLANT2H) if b=='\\' else
                             (PX_HALFSLANT2V if l=='\\' else PX_HALF2))
                    elif px == 'P':
                        v = ((None           if r=='/' else PX_HALFSLANT3H) if b=='/' else
                             (PX_HALFSLANT3V if r=='/' else PX_HALF3))
                    elif px == 'd':
                        v = ((None           if l=='/' else PX_HALFSLANT4H) if t=='/' else
                             (PX_HALFSLANT4V if l=='/' else PX_HALF4))
                    else:
                        tfull = (t in FILLED)
                        bfull = (b in FILLED)
                        lfull = (l in FILLED)
                        rfull = (r in FILLED)
                        if px == '\\':
                            if (lfull or bfull) and not (rfull and tfull):
                                v = ((None       if l=='b' else PX_SLANT1H) if b=='b' else
                                     (PX_SLANT1V if l=='b' else PX_HALF1))
                            if not (lfull and bfull) and (rfull or tfull):
                                v = ((None       if r=='9' else PX_SLANT2H) if t=='9' else
                                     (PX_SLANT2V if r=='9' else PX_HALF2))
                        elif px == '/':
                            if (lfull or tfull) and not (rfull and bfull):
                                v = ((None       if l=='P' else PX_SLANT3H) if t=='P' else
                                     (PX_SLANT3V if l=='P' else PX_HALF3))
                            if not (lfull and tfull) and (rfull or bfull):
                                v = ((None       if r=='d' else PX_SLANT4H) if b=='d' else
                                     (PX_SLANT4V if r=='d' else PX_HALF4))
                        elif px in '>)':
                            if lfull and not (rfull or tfull or bfull): v = PX_QUAD1
                            if not lfull and (rfull or tfull or bfull): v = PX_INVQUAD1
                        elif px in 'vu':
                            if tfull and not (lfull or rfull or bfull): v = PX_QUAD2
                            if not tfull and (lfull or rfull or bfull): v = PX_INVQUAD2
                        elif px in '<(':
                            if rfull and not (lfull or tfull or bfull): v = PX_QUAD3
                            if not rfull and (lfull or tfull or bfull): v = PX_INVQUAD3
                        elif px in '^n':
                            if bfull and not (lfull or rfull or tfull): v = PX_QUAD4
                            if not bfull and (lfull or rfull or tfull): v = PX_INVQUAD4
                        else:
                            raise ParseError(u'unknown %r pixel at %r' % (px, (rr, cc)))
                    if v is None:
                        raise ParseError(u'ambiguous %r pixel at %r' % (px, (rr, cc)))
                    if px in FILLED:
                        v |= PX_FULL
                    pixels.append(v)
            return pixels

        def flush_glyph(name, lines, parts):
            if not lines and not parts:
                raise ParseError(u'data for glyph %s is missing' % glyph_name(name))

            placeholder_bboxes = {}
            bbox = None

            def update_bbox(prev, r, c):
                if not prev: return r, c, r, c
                return min(prev[0],r), min(prev[1],c), max(prev[2],r), max(prev[3],c)

            subglyphs = []
            if lines:
                # strip placeholders and out-of-bbox markers (+)
                placeholders = set(ch for _, ch, _, _ in parts if ch)
                newlines = []
                for r, line in enumerate(lines):
                    newline = []
                    for c, px in enumerate(line):
                        if px in placeholders:
                            placeholder_bboxes[px] = update_bbox(placeholder_bboxes.get(px), r, c)
                            px = '!'
                        if px != '+':
                            bbox = update_bbox(bbox, r, c)
                        newline.append(px)
                    newlines.append(''.join(newline) + '.')
                newlines.append('.' * len(newlines[0]))
                lines = newlines

                if not bbox:
                    raise ParseError(u'glyph for %s is empty' % glyph_name(name))
                try:
                    pixels = parse_pixels(lines, bbox)
                except ParseError as e:
                    raise ParseError(u'parsing glyph for %s failed: %s' % (glyph_name(name), e))

                # this may be empty if entire glyph consists of subglyphs.
                top, left, bottom, right = bbox
                height = bottom - top + 1
                width = right - left + 1
                subglyphs.append(Subglyph(top=0, left=0, height=height, width=width,
                                          stride=width, data=pixels))

            # we now have proper relocation infos for subglyphs if any
            roff = bbox[0] if bbox else 0
            coff = bbox[1] if bbox else 0
            for subname, placeholder, subroff, subcoff in parts:
                if placeholder in placeholder_bboxes:
                    subtop, subleft, subbottom, subright = placeholder_bboxes[placeholder]
                    subheight = subbottom - subtop + 1
                    subwidth = subright - subleft + 1
                    subglyphs.append(Subglyph(top=subtop-roff+(subroff or 0),
                                              left=subleft-coff+(subcoff or 0),
                                              width=subwidth, height=subheight,
                                              stride=None, data=subname))
                else:
                    subglyphs.append(Subglyph(top=None if subroff is None else Delta(subroff),
                                              left=None if subcoff is None else Delta(subcoff),
                                              width=None, height=None, stride=None, data=subname))

            self.glyphs[name] = Glyph(height=None, width=None,
                                      preferred_top=roff, preferred_left=coff, subglyphs=subglyphs)

        current_glyph = None # or (code, lines, list of (subglyphs, placeholder ch or None))
        prev_args = []
        for line in fp:
            line = line.decode('utf-8')
            line, _, comment = (u' ' + u' '.join(line.split())).partition(u' //')
            line = line.strip()
            if not line: continue
            if len(line) == 1: line = u'glyph U+%04x' % ord(line)
            args = line.split()
            if args[-1] == '..': # continuation token
                prev_args.extend(args[:-1])
                continue
            else:
                args = prev_args + args
                prev_args = []
            if args[0] == 'glyph':
                if current_glyph:
                    flush_glyph(*current_glyph)
                    current_glyph = None
                name = parse_glyph_name(args[1])
                if isinstance(name, (int, basestring)):
                    # single glyph definition
                    if name in self.glyphs:
                        raise ParseError(u'duplicate glyph %s' % glyph_name(name))
                    if len(args) > 3 and args[2] == '=': # combined glyph
                        current_glyph = name, [], map(parse_subglyph_spec, args[3:])
                        placeholder_chars = [ch for _, ch, _, _ in current_glyph[2] if ch]
                        if len(placeholder_chars) != len(set(placeholder_chars)):
                            raise ParseError(u'duplicate placeholder characters for glyph %s' %
                                             glyph_name(name))
                    elif len(args) == 2:
                        current_glyph = name, [], []
                    else:
                        raise ParseError(u'unexpected arguments to `glyph`')
                else:
                    # multiple glyph definition (combined glyph only)
                    names = name
                    for name in names:
                        if name in self.glyphs:
                            raise ParseError(u'duplicate glyph %s' % glyph_name(name))
                    if len(args) > 3 and args[2] == '=':
                        subglyph_spec = map(parse_subglyph_spec, args[3:])
                        placeholder_chars = [ch for _, ch, _, _ in subglyph_spec if ch]
                        if len(placeholder_chars) != len(set(placeholder_chars)):
                            raise ParseError(u'duplicate placeholder characters for glyph %s' %
                                             args[1])
                        for i, name in enumerate(names):
                            spec = [(n if isinstance(n, (int, basestring)) else n[i%len(n)],
                                     pch, roff, coff) for n, pch, roff, coff in subglyph_spec]
                            flush_glyph(name, [], spec)
                    else:
                        raise ParseError(u'unexpected arguments to `glyph`')
            else:
                if not current_glyph:
                    raise ParseError(u'no glyph currently active')
                if len(args) == 1:
                    if current_glyph[1] and len(current_glyph[1][0]) != len(args[0]):
                        raise ParseError(u'inconsistent glyph width for %s' %
                                         glyph_name(current_glyph[0]))
                    current_glyph[1].append(args[0])
        if current_glyph: flush_glyph(*current_glyph)

    def resolve_glyphs(self):
        resolved = set()
        def resolve(name):
            if name in resolved: return
            resolved.add(name)

            gg = self.glyphs[name]
            assert isinstance(gg.preferred_top, int)
            assert isinstance(gg.preferred_left, int)

            if gg.height is None or gg.width is None:
                maxbottom = 0
                maxright = 0

                for k, g in enumerate(gg.subglyphs):
                    top, left, height, width, stride, data = g
                    if isinstance(data, list):
                        # every other field should be final
                        assert isinstance(top, int)
                        assert isinstance(left, int)
                        assert isinstance(height, int)
                        assert isinstance(width, int)
                        assert isinstance(stride, int)
                    else:
                        gg2 = self.glyphs[data]

                        # if gg2 doesn't have enough info to resolve g's fields, try to resolve
                        if not ((isinstance(top, int) or gg2.preferred_top is not None) and
                                (isinstance(left, int) or gg2.preferred_left is not None) and
                                (isinstance(height, int) or gg2.height is not None) and
                                (isinstance(width, int) or gg2.width is not None)):
                            resolve(data)
                            gg2 = self.glyphs[data] # since gg2 is, eh, an immutable tuple

                        # if the resolution has failed, it has a cyclic dependency
                        if not ((isinstance(top, int) or gg2.preferred_top is not None) and
                                (isinstance(left, int) or gg2.preferred_left is not None) and
                                (isinstance(height, int) or gg2.height is not None) and
                                (isinstance(width, int) or gg2.width is not None)):
                            raise ParseError(u'glyph %s has a cyclic dependency' %
                                             glyph_name(data))

                        if top is None: top = gg2.preferred_top
                        elif isinstance(top, Delta): top = gg2.preferred_top + top.value
                        if left is None: left = gg2.preferred_left
                        elif isinstance(left, Delta): left = gg2.preferred_left + left.value
                        if height is None: height = gg2.height
                        if width is None: width = gg2.width
                        gg.subglyphs[k] = Subglyph(top=top, left=left,
                                                   height=height, width=width,
                                                   stride=stride, data=data)

                    maxbottom = max(maxbottom, top + height)
                    maxright = max(maxright, left + width)

                self.glyphs[name] = gg._replace(
                    height=maxbottom if gg.height is None else gg.height,
                    width=maxright if gg.width is None else gg.width)

        for name in self.glyphs.keys(): resolve(name)

    def get_subglyphs(self, name):
        def collect(g, roff, coff, acc):
            if isinstance(g.data, list):
                acc.append(g._replace(top=g.top+roff, left=g.left+coff))
            else:
                gg = self.glyphs[g.data]
                for g2 in gg.subglyphs: collect(g2, g.top+roff, g.left+coff, acc)

        acc = []
        gg = self.glyphs[name]
        for g in gg.subglyphs: collect(g, gg.preferred_top, gg.preferred_left, acc)
        return acc

    def write_html(self, fp):
        contour_cache = {PX_FULL: {}, PX_SUBPIXEL: {}}
        def print_pixels(g, mask, cache=True):
            borders = None
            if cache:
                try:
                    key = g.height, g.width, g.stride, id(g.data)
                    borders = contour_cache[mask][key]
                except KeyError: pass
            if not borders:
                borders = track_contour(g.height, g.width, g.stride, g.data, mask)
                if cache: contour_cache[mask][key] = borders

            if borders:
                pathstr = []
                for p in borders:
                    x0, y0 = p[0]
                    pathstr.append('M%s %s' % (x0 + g.left, y0 + g.top))
                    for i in xrange(1, len(p)):
                        x, y = p[i]
                        if y == y0: pathstr.append('h%s' % (x - x0))
                        elif x == x0: pathstr.append('v%s' % (y - y0))
                        else: pathstr.append('l%s %s' % (x - x0, y - y0))
                        x0 = x; y0 = y
                    pathstr.append('z')
                path = ''.join(pathstr)
                print >>fp, '<path d="{path}" fill="#{color:06x}" />'.format(
                    path = path, color = (hash(path) & 0x7f7f7f) + 0x808080,
                )

        def print_svg(name, scale, ignore_subpixel):
            gg = self.glyphs[name]
            height = gg.height
            width = gg.width
            glyphs = self.get_subglyphs(name)

            mask = PX_FULL if ignore_subpixel else PX_SUBPIXEL

            print >>fp, '<svg viewBox="0 0 {w} {h}" width="{ww}" height="{hh}">'.format(
                w = width, h = height,
                ww = width * scale, hh = height * scale,
            )
            if False and ignore_subpixel: # no cache slows things down
                print_pixels(merge_subglyphs_sans_subpixel(height, width, glyphs), mask,
                             cache=False)
            else:
                for g in glyphs: print_pixels(g, mask)
            fp.write('</svg>')

        all_glyphs = self.glyphs.keys()
        num_glyphs = len(all_glyphs)
        num_pub_glyphs = len([k for k in all_glyphs if isinstance(k, int)])

        print >>fp, '<!doctype html>'
        print >>fp, '<html><head><meta charset="utf-8" /><title>Unison: graphic sample</title><style>'
        print >>fp, 'body{background:black;color:white}div{color:gray}#sampleglyphs{display:none}body.sample #sampleglyphs{display:block}body.sample #glyphs{display:none}'
        print >>fp, 'svg{background:#111;fill:white;vertical-align:top}:target svg{background:#333}svg:hover path,body.sample svg path{fill:white!important}a svg path{fill:gray!important}'
        print >>fp, '</style></head><body>'
        print >>fp, '<input id="sample" placeholder="Input sample text here" size="40"> <input type="reset" id="reset" value="Reset"> | %d characters, %d intermediate glyphs so far | <a href="sample.png">PNG</a> | <a href="live.html">live</a>' % (num_pub_glyphs, num_glyphs - num_pub_glyphs)
        print >>fp, '<hr /><div id="sampleglyphs"></div><div id="glyphs">'
        for name in sorted(k for k in all_glyphs if isinstance(k, int)):
            fp.write('<a href="#u%x"><span id="sm-u%x" title="%s">' % (name, name, escape(glyph_name(name))))
            print_svg(name, 1, True)
            fp.write('</span></a>')
        print >>fp, '<hr />'
        for _, name in sorted((unicodedata.normalize('NFD', unichr(k)), k) for k in all_glyphs if isinstance(k, int)):
            fp.write('<span id="u%x" title="%s">' % (name, escape(glyph_name(name))))
            print_svg(name, 5, False)
            fp.write('</span>')
        if False:
            print >>fp, '<hr />'
            for name in sorted(k for k in all_glyphs if not isinstance(k, int)):
                fp.write('<span id="glyph-%s" title="%s">' % (name, name))
                print_svg(name, 5, False)
                fp.write('</span>')
        print >>fp, '</div><script>'
        print >>fp, 'function $(x){return document.getElementById(x)}'
        print >>fp, 'function f(t){if(t.normalize)t=t.normalize();document.body.className=t?"sample":"";var sm="",bg="";for(var i=0;i<t.length;++i){var c=t.charCodeAt(i).toString(16);sm+=($("sm-u"+c)||{}).innerHTML||t[i];bg+=($("u"+c)||{}).innerHTML||t[i]}$("sampleglyphs").innerHTML=sm+"<hr /><span style=font-size:500%>"+bg+"</span>"}'
        print >>fp, '$("sample").onchange=$("sample").onkeyup=function(e){f(this.value)}'
        print >>fp, '$("reset").onclick=function(){$("sample").value="";f("")}'
        print >>fp, '</script></body></html>'

    def write_pgm(self, fp):
        MAX_HEIGHT = 16
        LINE_WIDTH = 256
        NUM_GLYPHS_PER_LINE = (32, 16, 8, 4, 2, 1)

        # determine the width of glyphs (and check the height)
        glyphs = {} # (width, height, list of resolved subglyphs)
        unavailable_widths = set() # (# glyphs per line, start character)
        for name in self.glyphs.keys():
            if not isinstance(name, int): continue
            gg = self.get_subglyphs(name)
            width = max(g.left + g.width for g in gg)
            height = max(g.top + g.height for g in gg)
            assert height <= MAX_HEIGHT, 'glyph %s is too tall' % glyph_name(name)
            assert width <= LINE_WIDTH, 'glyph %s is too wide' % glyph_name(name)
            glyphs[name] = width, height, gg
            for w in NUM_GLYPHS_PER_LINE:
                if width > LINE_WIDTH // w:
                    unavailable_widths.add((w, name & -w))

        # determine the position of each glyph
        last = None
        row = -1
        gap = 0
        positions = {} # (row, column)
        row_starts = []
        row_offset = [] # i.e. accumulated `gap`
        all_names = sorted(k for k in self.glyphs.keys() if isinstance(k, int))
        for name in all_names:
            for w in NUM_GLYPHS_PER_LINE:
                if (w, name & -w) not in unavailable_widths: break
            else: assert False
            current = w, name & -w
            if last != current:
                if current[1] - (last or (0, 0))[1] > 32: gap += 8
                row += 1
                row_starts.append(name & -w)
                row_offset.append(gap)
                last = current
            positions[name] = row, (name & (w-1)) * (LINE_WIDTH // w)
        nrows = row + 1

        #          +---------------------+ ^
        # __U+xxxx | a b c d e f g h ... | | 17px each + last one
        # <------> +---------------------+ v
        #  8*8px+1 ^ <-----------------> ^
        #         1px      8*32px       1px
        imwidth = 8*8 + 1 + 1 + LINE_WIDTH + 1
        imheight = (MAX_HEIGHT + 1) * nrows + 1 + gap
        imline = bytearray('\xff') * (8*8 + 1) + bytearray('\x80') * (1 + LINE_WIDTH + 1)

        def render_glyphs(current, left, (width, height, glyphs), color):
            for r in xrange(height):
                for c in xrange(left, left + width):
                    current[r][c] = 255
            for g in glyphs:
                for r in xrange(g.height):
                    for c in xrange(g.width):
                        if g.data[r*g.stride+c] & PX_FULL:
                            current[g.top+r][g.left+left+c] = color

        fp.write('P5 %d %d 255\n' % (imwidth, imheight))
        fp.write(imline)
        row = -1
        current = None
        lastlabel = None
        for name in all_names:
            r, left = positions[name]
            if r != row:
                assert r - row == 1
                row = r
                if current:
                    for line in current: fp.write(line)
                for i in xrange(row_offset[row-1] if row>0 else 0, row_offset[row]):
                    fp.write(imline)
                current = [imline[:] for i in xrange(MAX_HEIGHT + 1)]
                color = 128
                if lastlabel != (row_starts[row] & -32):
                    lastlabel = (row_starts[row] & -32)
                    color = 0
                label = '%8s' % ('U+%04X' % row_starts[row])
                for i, ch in enumerate(label):
                    if ch == ' ': continue
                    render_glyphs(current, i * 8, glyphs[ord(ch)], color)
            render_glyphs(current, 8*8 + 1 + 1 + left, glyphs[name], 0)
        if current:
            for line in current: fp.write(line)

    def write_live_html(self, fp):
        print >>fp, '<!doctype html>'
        print >>fp, '<html><head><meta charset="utf-8" /><title>Unison: live sample</title><style>'
        print >>fp, '@font-face{font-family:Unison;src:url(unison.ttf)}pre{font-family:Unison,monospace;font-size:200%;line-height:1;margin:0;white-space:pre-wrap}'
        print >>fp, '</style><script>window.onload=function(){document.designMode="on"}</script>'
        print >>fp, '</head><body><pre>'
        print >>fp, 'Hello? This is the <u>Unison</u> font.'
        print >>fp, 'You can play with it right here or download it <a href="unison.ttf">here</a>.'
        print >>fp, 'Please note that this is in development and subject to change.'
        print >>fp
        print >>fp, '┌────────────────────┐'
        print >>fp, '│All Supported Glyphs│'
        print >>fp, '└────────────────────┘'
        print >>fp
        chars = []
        for name in sorted(k for k in self.glyphs.keys() if isinstance(k, int)):
            if chars and (chars[0] >> 4) != (name >> 4):
                print >>fp, escape(u''.join(map(unichr, chars)))
                chars = []
            chars.append(name)
        if chars: print >>fp, escape(u''.join(map(unichr, chars)))
        print >>fp, '</pre></body></html>'

    def write_ttx(self, fp):
        SCALE = 16

        def get_subname(name):
            if isinstance(name, int):
                return 'uni{name:04X}'.format(name=name)
            else:
                return name

        # left-side bearing cannot be easily calculated without a recursion
        lsbs = {}
        def get_lsb_from_pixels(height, width, stride, data):
            return min(c + LEFTMOST[data[r*stride+c] & PX_SUBPIXEL] if data[r*stride+c] else width
                       for r in xrange(height) for c in xrange(width))
        def get_lsb(name):
            try: return lsbs[name]
            except KeyError:
                gg = self.glyphs[name]
                lsb = gg.width
                for g in gg.subglyphs:
                    if isinstance(g.data, list):
                        glsb = get_lsb_from_pixels(g.height, g.width, g.stride, g.data)
                    else:
                        glsb = get_lsb(g.data)
                    lsb = min(lsb, g.left + glsb)
                lsbs[name] = lsb
                return lsb

        # OpenType requires that every glyph is either simple or composite.
        # since some glyphs are hybrid, we need to remap them.
        subnames = []
        hasnotdef = False
        for name, gg in sorted(self.glyphs.items()):
            if name == '.notdef': hasnotdef = True
            subname = get_subname(name)
            for i, g in enumerate(gg.subglyphs):
                if not isinstance(g.data, list): continue
                subnames.append(('%s-%d' % (subname, i), g.width,
                                 get_lsb_from_pixels(g.height, g.width, g.stride, g.data)))
            subnames.append((subname, gg.width, get_lsb(name)))
        assert hasnotdef, '.notdef glyph is undefined, will cause a bad effect including ' \
                          'a missing glyph for the first character (generally U+0020)'

        # Windows and OS X has a different idea about the typographic metrics
        # https://people.gnome.org/~fejj/code/lineheight.c
        emsize = int(self.height * SCALE)
        ascent = int(self.ascent * SCALE)
        descent = int(self.descent * SCALE)
        linegap = int((self.height - self.ascent - self.descent) * SCALE)

        # let's start over
        # (the order of the following sections should not be changed, OS X complains a lot)
        print >>fp, '<?xml version="1.0" encoding="UTF-8"?>'
        print >>fp, r'<ttFont sfntVersion="\x00\x01\x00\x00" ttLibVersion="3.0">'

        # internal glyph order (seems to have to be the first tag!)
        print >>fp, '<GlyphOrder>'
        print >>fp, '<GlyphID name=".notdef"/>'
        for subname, _, _ in subnames:
            if subname == '.notdef': continue
            print >>fp, '<GlyphID name="{name}"/>'.format(name=subname)
        print >>fp, '</GlyphOrder>'

        # hhea
        print >>fp, '<hhea>'
        print >>fp, '<tableVersion value="1.0"/>'
        # ascent + (-descent) + linegap = line height
        print >>fp, '<ascent value="{ascent}"/>'.format(ascent=ascent)
        print >>fp, '<descent value="{descent}"/>'.format(descent=-descent)
        print >>fp, '<lineGap value="{linegap}"/>'.format(linegap=linegap)
        print >>fp, '<minLeftSideBearing value="-1"/>'
        print >>fp, '<minRightSideBearing value="-50"/>'
        print >>fp, '<xMaxExtent value="1641"/>'
        print >>fp, '<caretSlopeRise value="1"/>'
        print >>fp, '<caretSlopeRun value="0"/>'
        print >>fp, '<caretOffset value="0"/>'
        print >>fp, '<reserved0 value="0"/>'
        print >>fp, '<reserved1 value="0"/>'
        print >>fp, '<reserved2 value="0"/>'
        print >>fp, '<reserved3 value="0"/>'
        print >>fp, '<metricDataFormat value="0"/>'
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<advanceWidthMax value="0"/>'
        print >>fp, '</hhea>'

        # head
        print >>fp, '<head>'
        print >>fp, '<magicNumber value="0x5f0f3cf5"/>'
        print >>fp, '<fontRevision value="1.0"/>'
        print >>fp, '<unitsPerEm value="{emsize}"/>'.format(emsize=emsize)
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<tableVersion value="1.0"/>'
        print >>fp, '<checkSumAdjustment value="0"/>'
        print >>fp, '<flags value="00000000 00001011"/>'
        print >>fp, '<created value="Sun Sep 16 23:28:06 2007"/>'
        print >>fp, '<modified value="Sun Sep 16 23:28:06 2007"/>'
        print >>fp, '<xMin value="0"/>'
        print >>fp, '<yMin value="0"/>'
        print >>fp, '<xMax value="0"/>'
        print >>fp, '<yMax value="0"/>'
        print >>fp, '<macStyle value="00000000 00000000"/>'
        print >>fp, '<lowestRecPPEM value="8"/>'
        print >>fp, '<fontDirectionHint value="2"/>'
        print >>fp, '<indexToLocFormat value="1"/>'
        print >>fp, '<glyphDataFormat value="0"/>'
        print >>fp, '</head>'

        # maxp
        print >>fp, '<maxp>'
        print >>fp, '<tableVersion value="0x10000"/>'
        print >>fp, '<maxZones value="2"/>'
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<numGlyphs value="0"/>'
        print >>fp, '<maxPoints value="0"/>'
        print >>fp, '<maxContours value="0"/>'
        print >>fp, '<maxCompositePoints value="0"/>'
        print >>fp, '<maxCompositeContours value="0"/>'
        print >>fp, '<maxTwilightPoints value="0"/>'
        print >>fp, '<maxStorage value="0"/>'
        print >>fp, '<maxFunctionDefs value="0"/>'
        print >>fp, '<maxInstructionDefs value="0"/>'
        print >>fp, '<maxStackElements value="0"/>'
        print >>fp, '<maxSizeOfInstructions value="0"/>'
        print >>fp, '<maxComponentElements value="0"/>'
        print >>fp, '<maxComponentDepth value="0"/>'
        print >>fp, '</maxp>'

        # OS/2
        print >>fp, '<OS_2>'
        print >>fp, '<version value="1"/>'
        print >>fp, '<xAvgCharWidth value="{width}"/>'.format(width=int(8*SCALE))
        print >>fp, '<usWeightClass value="400"/>'
        print >>fp, '<usWidthClass value="5"/>'
        print >>fp, '<fsType value="00000000 00001000"/>'
        print >>fp, '<ySubscriptXSize value="390"/>'
        print >>fp, '<ySubscriptYSize value="419"/>'
        print >>fp, '<ySubscriptXOffset value="0"/>'
        print >>fp, '<ySubscriptYOffset value="84"/>'
        print >>fp, '<ySuperscriptXSize value="390"/>'
        print >>fp, '<ySuperscriptYSize value="419"/>'
        print >>fp, '<ySuperscriptXOffset value="0"/>'
        print >>fp, '<ySuperscriptYOffset value="287"/>'
        print >>fp, '<yStrikeoutSize value="29"/>'
        print >>fp, '<yStrikeoutPosition value="155"/>'
        print >>fp, '<sFamilyClass value="0"/>'
        print >>fp, '<panose>'
        print >>fp, '  <bFamilyType value="3"/>'
        print >>fp, '  <bSerifStyle value="11"/>'
        print >>fp, '  <bWeight value="6"/>'
        print >>fp, '  <bProportion value="0"/>'
        print >>fp, '  <bContrast value="0"/>'
        print >>fp, '  <bStrokeVariation value="1"/>'
        print >>fp, '  <bArmStyle value="1"/>'
        print >>fp, '  <bLetterForm value="1"/>'
        print >>fp, '  <bMidline value="1"/>'
        print >>fp, '  <bXHeight value="1"/>'
        print >>fp, '</panose>'
        print >>fp, '<ulUnicodeRange1 value="10000000 00000000 00000000 10101111"/>'
        print >>fp, '<ulUnicodeRange2 value="00000001 10010001 00100000 01001010"/>'
        print >>fp, '<ulUnicodeRange3 value="00000000 00000000 00000000 00000000"/>'
        print >>fp, '<ulUnicodeRange4 value="00000000 00000000 00000000 00000000"/>'
        print >>fp, '<achVendID value="Morg"/>'
        print >>fp, '<fsSelection value="00000000 10000000"/>'
        # sTypoAscender + (-sTypoDescender) + sTypoLineGap = line height
        # this is forced by setting bit 7 of fsSelection above
        print >>fp, '<sTypoAscender value="{ascender}"/>'.format(ascender=ascent)
        print >>fp, '<sTypoDescender value="{descender}"/>'.format(descender=-descent)
        print >>fp, '<sTypoLineGap value="{linegap}"/>'.format(linegap=linegap)
        # ascent + descent = (de facto) line height
        # actually the size of clipping region, but also (incorrectly) used as line height
        # try to be consistent with above
        print >>fp, '<usWinAscent value="{ascent}"/>'.format(ascent=linegap+ascent)
        print >>fp, '<usWinDescent value="{descent}"/>'.format(descent=descent)
        print >>fp, '<ulCodePageRange1 value="01100000 00101000 00000000 00010001"/>'
        print >>fp, '<ulCodePageRange2 value="10000001 11010100 00000000 00000000"/>'
        print >>fp, '</OS_2>'

        # hmtx
        print >>fp, '<hmtx>'
        for subname, subwidth, sublsb in subnames:
            if sublsb >= subwidth: sublsb = 0 # special casing for spaces
            print >>fp, '<mtx name="{name}" width="{width}" lsb="{lsb}"/>'.format(
                    name=subname, width=int(subwidth*SCALE), lsb=int(sublsb*SCALE))
        print >>fp, '</hmtx>'

        # cmap
        print >>fp, '<cmap>'
        print >>fp, '<tableVersion version="0"/>'
        for platid, platenc in ((0, 3), (1, 0)):
            print >>fp, '<cmap_format_4 platformID="{platid}" platEncID="{platenc}" ' \
                         'language="0">'.format(platid=platid, platenc=platenc)
            for name in sorted(k for k in self.glyphs.keys() if isinstance(k, int)):
                print >>fp, '<map code="{name:#x}" name="uni{name:04X}"/>'.format(name=name)
            print >>fp, '</cmap_format_4>'
        print >>fp, '</cmap>'

        # loca
        print >>fp, '<loca/>'

        # fpgm
        print >>fp, '<fpgm>'
        #print >>fp, '<bytecode></bytecode>'
        print >>fp,'''<bytecode>
      40120807 06050403 021b1a19 18171615
      141e1f20 2c18b003 3f2d2c18 b0023f2d
      2c18b001 3f2d2cb0 004358b1 0000451e
      1f1bb000 1e592d2c b0004358 b1000045
      1e1f1bb0 001e592d 2cb00043 58b10000
      451e1f1b b0001e59 2d2cb000 4358b100
      00451e1f 1bb0001e 592d2cb0 004358b1
      0000451e 1f1bb000 1e592d2c b0004358
      b1000045 1e1f1bb0 001e592d 2cb00043
      58b10000 451e1f1b b0001e59 2d2cb000
      4358b100 00451e1f 1bb0001e 592d2c18
      2fdd2d2c 182f3cdd 2d2c182f 173cdd2d
      2c182fdd 3c2d2c18 2fdd173c 2d2c182f
      3cdd3c2d 2c182f17 3cdd173c 2d
      </bytecode>'''
        print >>fp, '</fpgm>'

        # prep
        print >>fp, '<prep>'
        #print >>fp, '<bytecode></bytecode>'
        print >>fp, '''<bytecode>
      4bb01050 58ba01d0 00040000 8d8d851b
      b901d000 008d8559 b20010aa 4b524b50
      5a42   
    </bytecode>'''
        print >>fp, '</prep>'

        # cvt
        print >>fp, '<cvt>'
        #print >>fp, '<cv index="0" value="0"/>' # dummy
        print >>fp, '''
    <cv index="0" value="8"/>
    <cv index="1" value="-9999"/>
    <cv index="2" value="-9999"/>
    <cv index="3" value="-9999"/>
    <cv index="4" value="20"/>
    <cv index="5" value="380"/>
    '''
        print >>fp, '</cvt>'

        # glyf
        print >>fp, '<glyf>'
        for name, gg in sorted(self.glyphs.items()):
            name = get_subname(name)

            # flush simple glyphs first
            for i, g in enumerate(gg.subglyphs):
                if not isinstance(g.data, list): continue
                subname = '%s-%d' % (name, i)
                print >>fp, '<TTGlyph name="{subname}">'.format(subname=subname)
                for contour in track_contour(g.height, g.width, g.stride, g.data, PX_SUBPIXEL):
                    print >>fp, '<contour>'
                    for x, y in contour:
                        y = g.height - y
                        print >>fp, '<pt x="{x}" y="{y}" on="1"/>'.format(x=int(x*SCALE), y=int(y*SCALE))
                    print >>fp, '</contour>'
                print >>fp, '<instructions><bytecode></bytecode></instructions>'
                print >>fp, '</TTGlyph>'

            print >>fp, '<TTGlyph name="{name}">'.format(name=name)
            for i, g in enumerate(gg.subglyphs):
                x = g.left
                y = g.top
                if isinstance(g.data, list):
                    subname = '%s-%d' % (name, i)
                    subheight = g.height
                else:
                    subname = get_subname(g.data)
                    subheight = g.height
                    if y is None: y = self.glyphs[g.data].preferred_top
                    if x is None: x = self.glyphs[g.data].preferred_left
                x = (x or 0)
                y = gg.height - ((y or 0) + subheight)
                print >>fp, '<component glyphName="{subname}" x="{x}" y="{y}" ' \
                             'flags="0x1004"/>'.format(subname=subname,
                                                       x=int(x*SCALE), y=int(y*SCALE))
            print >>fp, '<instructions><bytecode></bytecode></instructions>'
            print >>fp, '</TTGlyph>'
        print >>fp, '</glyf>'

        # name
        print >>fp, '<name>'
        names = [
            (0, 'copyright', u'Copyright (c) 2015, Kang Seonghoon. All Rights Reserved.'),
            (1, 'family', u'Unison'),
            (2, 'subfamily', u'Regular'),
            (3, 'identifier', u'Unison'),
            (4, 'fontname', u'Unison'),
            (5, 'version', u'Version 0.1'),
            (6, 'psname', u'Unison'),
        ]
        for nameid, _, nameval in names:
            print >>fp, '<naming nameId="{id}" platformID="0" platEncID="0" langID="0x0" ' \
                         'unicode="True">{val}</naming>'.format(id=nameid,
                                                                val=nameval.encode('utf-8'))
        print >>fp, '</name>'

        # post
        print >>fp, '<post>'
        print >>fp, '<formatType value="2.0"/>'
        print >>fp, '<italicAngle value="0.0"/>'
        print >>fp, '<underlinePosition value="{ulinepos}"/>'.format(ulinepos=descent)
        print >>fp, '<underlineThickness value="{ulinesize}"/>'.format(ulinesize=int(SCALE))
        print >>fp, '<isFixedPitch value="0"/>'
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<minMemType42 value="0"/>'
        print >>fp, '<maxMemType42 value="0"/>'
        print >>fp, '<minMemType1 value="0"/>'
        print >>fp, '<maxMemType1 value="0"/>'
        print >>fp, '<psNames/>'
        print >>fp, '<extraNames/>'
        print >>fp, '</post>'

        print >>fp, '</ttFont>'

if __name__ == '__main__':
    import sys, time
    font = Font()
    t1 = time.time()
    try:
        for path in sys.argv[1:]:
            with open(path) as f:
                font.read(f)
        font.resolve_glyphs()
    except ParseError as e:
        print >>sys.stderr, unicode(e).encode('utf-8')
        raise SystemExit(1)
    t2 = time.time()
    with open('sample.html', 'w') as f:
        font.write_html(f)
    with open('live.html', 'w') as f:
        font.write_live_html(f)
    with open('sample.pgm', 'w') as f:
        font.write_pgm(f)
    with open('unison.ttx', 'w') as f:
        font.write_ttx(f)
    t3 = time.time()
    print >>sys.stderr, '%.3fs parsing, %.3fs rendering' % (t2 - t1, t3 - t2)

