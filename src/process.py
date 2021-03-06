# coding: utf-8
#
# Copyright (c) 2015--2016, Kang Seonghoon.
#
# Permission is hereby granted, free of charge, to any person obtaining a
# copy of this software and associated documentation files (the "Software"),
# to deal in the Software without restriction, including without limitation
# the rights to use, copy, modify, merge, publish, distribute, sublicense,
# and/or sell copies of the Software, and to permit persons to whom the
# Software is furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
# THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
# FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
# DEALINGS IN THE SOFTWARE.

import time
import sys
import os
import os.path
import re
import glob
from fractions import gcd
import itertools
import urllib
import cPickle as pickle
import zipfile
from xml.etree import cElementTree as ET
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

PIXELNAMES = {
    ".":    PX_EMPTY,       ",":   PX_FULL | PX_EMPTY,
    "O":    PX_ALMOSTFULL,  "@":   PX_FULL | PX_ALMOSTFULL,
    "|\\":  PX_HALF1,       "|b":  PX_FULL | PX_HALF1,
    "\\|":  PX_HALF2,       "9|":  PX_FULL | PX_HALF2,
    "|/":   PX_HALF3,       "|P":  PX_FULL | PX_HALF3,
    "/|":   PX_HALF4,       "d|":  PX_FULL | PX_HALF4,
    "|>":   PX_QUAD1,       "|)":  PX_FULL | PX_QUAD1,
    "'v'":  PX_QUAD2,       "'u'": PX_FULL | PX_QUAD2,
    "<|":   PX_QUAD3,       "(|":  PX_FULL | PX_QUAD3,
    ".^.":  PX_QUAD4,       ".n.": PX_FULL | PX_QUAD4,
    ">|":   PX_INVQUAD1,    ")|":  PX_FULL | PX_INVQUAD1,
    "|v|":  PX_INVQUAD2,    "|u|": PX_FULL | PX_INVQUAD2,
    "|<":   PX_INVQUAD3,    "|(":  PX_FULL | PX_INVQUAD3,
    "|^|":  PX_INVQUAD4,    "|n|": PX_FULL | PX_INVQUAD4,
    "|\\.": PX_SLANT1H,     "|b.": PX_FULL | PX_SLANT1H,
    "'\\|": PX_SLANT2H,     "'9|": PX_FULL | PX_SLANT2H,
    "|/'":  PX_SLANT3H,     "|P'": PX_FULL | PX_SLANT3H,
    "./|":  PX_SLANT4H,     ".d|": PX_FULL | PX_SLANT4H,
    "i\\.": PX_SLANT1V,     "ib.": PX_FULL | PX_SLANT1V,
    "'\\!": PX_SLANT2V,     "'9!": PX_FULL | PX_SLANT2V,
    "!/'":  PX_SLANT3V,     "!P'": PX_FULL | PX_SLANT3V,
    "./i":  PX_SLANT4V,     ".di": PX_FULL | PX_SLANT4V,
    "|_\\": PX_HALFSLANT1H, "|_b": PX_FULL | PX_HALFSLANT1H,
    "\\_|": PX_HALFSLANT2H, "9_|": PX_FULL | PX_HALFSLANT2H,
    "|_/":  PX_HALFSLANT3H, "|_P": PX_FULL | PX_HALFSLANT3H,
    "/_|":  PX_HALFSLANT4H, "d_|": PX_FULL | PX_HALFSLANT4H,
    "|\\i": PX_HALFSLANT1V, "|bi": PX_FULL | PX_HALFSLANT1V,
    "!\\|": PX_HALFSLANT2V, "!9|": PX_FULL | PX_HALFSLANT2V,
    "|/!":  PX_HALFSLANT3V, "|P!": PX_FULL | PX_HALFSLANT3V,
    "i/|":  PX_HALFSLANT4V, "id|": PX_FULL | PX_HALFSLANT4V,
    "+":    PX_DOT,         "*":   PX_FULL | PX_DOT,
}

# glyph flags
G_STICKY = 1
G_INLINE = 2

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
Adjoin = namedtuple('Adjoin', 'value')

# data = None | list of pixel codes | subglyph name | Adjoin(subglyph name)
# top/left can be a Delta value instead of the actual value; will be resolved later.
# negated is 1 if the subglyph should be negated; >=2 matters for complex nesting.
Subglyph = namedtuple('Subglyph', 'top left height width stride data negated')

# height/width should be the intrinsic height/width, and may be filled by resolve_glyphs.
# preferred_top/left becomes the actual offset when the glyph is used at the top level.
# points are named points (most importantly, `-name`/`+name` for adjoining target/source points).
# note that all internal coordinates exclude preferred_top/left.
Glyph = namedtuple('Glyph', 'flags height width preferred_top preferred_left subglyphs points')

# all four fields contain a list of either a glyph name, a set name (preceded by %) or glyph list.
Remap = namedtuple('Remap', 'lookbehind pattern lookahead replacement')

class ParseError(ValueError): pass

UCD = namedtuple('UCD', 'name decomp')

class ExternalData(object):
    def __init__(self, cachepath, universion):
        self.cachepath = cachepath
        self.universion = universion
        self.unicodedata = None # UnicodeData.txt

    def download_if_not_exists(self, path, cached_path):
        try:
            cached_dir = os.path.dirname(cached_path)
            if cached_dir: os.makedirs(cached_dir)
        except Exception:
            pass
        if not os.path.exists(cached_path):
            print >>sys.stderr, 'downloading %s...' % path,
            try:
                urllib.urlretrieve(path, cached_path)
                print >>sys.stderr, 'done.'
            except:
                print >>sys.stderr, 'failed.'
                try: os.unlink(cached_path)
                except OSError: pass
                raise

    def try_fetch_cache(self, filename, get):
        path = os.path.join(self.cachepath, filename)
        try:
            with open(path, 'rb') as f:
                return pickle.load(f)
        except Exception:
            data = get()
            with open(path, 'wb') as f:
                pickle.dump(data, f, protocol=pickle.HIGHEST_PROTOCOL)
            return data

    def parse_unicodedata(self):
        ucdpath = os.path.join(self.cachepath, 'UnicodeData.txt')
        self.download_if_not_exists('http://www.unicode.org/Public/' + self.universion +
                                    '/ucd/UnicodeData.txt', ucdpath)
        data = {}
        with open(ucdpath) as f:
            for line in f:
                code, name, category, ccc, bidi, decomp, decimal, digit, numeric, \
                mirrored, u1name, isocomment, upperch, lowerch, titlech = \
                    line.rstrip('\r\n').split(';')
                code = int(code, 16)
                if not name.startswith('<'):
                    decompty, _, decompmap = decomp.rpartition('> ')
                    decompty = decompty[1:]
                    decompmap = [int(c, 16) for c in decompmap.split()]
                    if not decompty: decompty = None
                    if not decompmap: decompmap = None
                    data[code] = UCD(name=name, decomp=(decompty, decompmap))
        return data

    def get_unicodedata(self):
        if not self.unicodedata:
            self.unicodedata = self.try_fetch_cache('unicodedata.dat', self.parse_unicodedata)
            assert self.unicodedata
        return self.unicodedata

    def get_char_name(self, code):
        # algorithmic mappings
        if 0x3400 <= code <= 0x4db5 or 0x4e00 <= code <= 0x9fd5 or 0x20000 <= code <= 0x2a6d6 or \
           0x2a700 <= code <= 0x2b734 or 0x2b740 <= code <= 0x2b81d or 0x2b820 <= code <= 0x2cea1:
            return 'CJK UNIFIED IDEOGRAPH-%X' % code
        if 0xac00 <= code <= 0xd7a3:
            a, bc = divmod(code - 0xac00, 588)
            b, c = divmod(bc, 28)
            return 'HANGUL SYLLABLE %s%s%s' % (
                ('G','GG','N','D','DD','R','M','B','BB','S','SS','','J','JJ','C','K',
                 'T','P','H')[a],
                ('A','AE','YA','YAE','EO','E','YEO','YE','O','WA','WAE','OE','YO','U','WEO','WE',
                 'WI','YU','EU','YI','I')[b],
                ('','G','GG','GS','N','NJ','NH','D','L','LG','LM','LB','LS','LT','LP','LH',
                 'M','B','BS','S','SS','NG','J','C','K','T','P','H')[c],
            )

        entry = self.get_unicodedata().get(code, None)
        if not entry: return None
        return entry.name

ExternalData = ExternalData(
    cachepath=os.path.join(os.path.dirname(__file__), '..', 'cache'),
    universion='8.0.0')

def lcm(a, b):
    return a // gcd(a, b) * b

def escape(s):
    return s.encode('utf-8').replace('&', '&amp;').replace('"', '&quot;') \
                            .replace('<', '&lt;').replace('>', '&gt;')

if sys.maxunicode < 0x10ffff:
    def try_ord(s):
        if len(s) == 1: return ord(s)
        if len(s) == 2 and u'\ud800' <= s[0] <= u'\udbff' and u'\udc00' <= s[1] <= u'\udfff':
            return 0x10000 + ((ord(s[0]) & 0x3ff) << 10) + (ord(s[1]) & 0x3ff)
        return None

    def unichar(c):
        if c < 0x10000: return unichr(c)
        c -= 0x10000
        return unichr(0xd800 + (c >> 10)) + unichr(0xdc00 + (c & 0x3ff))
else:
    def try_ord(s):
        if len(s) == 1: return ord(s)
        return None

    unichar = unichr

def char_name(i):
    name = ExternalData.get_char_name(i) or u''
    if name: name = u' ' + name
    return u'U+%04X%s (%s)' % (i, name, unichar(i))

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

def inside(x1, y1, x, y, x2, y2):
    # i.e. colinear and (x,y) is inside a rectangle formed by (x1,y1) and (x2,y2)
    return ccw(x1, y1, x, y, x2, y2) == 0 and (x1 <= x <= x2 or x1 >= x >= x2) \
                                          and (y1 <= y <= y2 or y1 >= y >= y2)

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
                    if (linesegs & 0b11000000) == 0b11000000: segs.append((x, y, x + 1, y))
                    else:
                        if linesegs & 0b10000000: segs.append((x, y, x + 0.5, y))
                        if linesegs & 0b01000000: segs.append((x + 0.5, y, x + 1, y))
                    if (linesegs & 0b00110000) == 0b00110000: segs.append((x + 1, y, x + 1, y + 1))
                    else:
                        if linesegs & 0b00100000: segs.append((x + 1, y, x + 1, y + 0.5))
                        if linesegs & 0b00010000: segs.append((x + 1, y + 0.5, x + 1, y + 1))
                    if (linesegs & 0b00001100) == 0b00001100: segs.append((x + 1, y + 1, x, y + 1))
                    else:
                        if linesegs & 0b00001000: segs.append((x + 1, y + 1, x + 0.5, y + 1))
                        if linesegs & 0b00000100: segs.append((x + 0.5, y + 1, x, y + 1))
                    if (linesegs & 0b00000011) == 0b00000011: segs.append((x, y + 1, x, y))
                    else:
                        if linesegs & 0b00000010: segs.append((x, y + 1, x, y + 0.5))
                        if linesegs & 0b00000001: segs.append((x, y + 0.5, x, y))

                    if ~pixel & disconnected:
                        segs.extend((x + x1, y + y1, x + x2, y + y2) for x1, y1, x2, y2 in gapsegs)

            assert len(segs) == len(set(segs)), segs
            pxtosegs = {}
            for x1, y1, x2, y2 in segs:
                pxtosegs.setdefault((x1, y1), []).append((x2, y2))
                pxtosegs.setdefault((x2, y2), []).append((x1, y1))

            while pxtosegs:
                (x0, y0), v = pxtosegs.popitem()
                assert len(v) >= 2
                assert len(v) % 2 == 0
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
                            assert len(nx) % 2 == 0
                            if nx: pxtosegs[x, y] = nx
                            break

                        for k, v in indices.items():
                            if v >= len(path): del k

                        # simulate the state right after (x, y) was originally added
                        px, py = path[-1]
                        dx = x - px
                        dy = y - py

                    xx, yy = nx.pop(0)
                    assert len(nx) % 2 == 0
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
            for k in xrange(len(path)):
                # it is possible that every point in `path` overlaps with `p`
                # while they don't share any common points themselves.
                # thus we cheat here and try to pick a point that is not in `path`
                # while it *does* overlap with `path` and is very unlikely to overlap with `p`.
                # this is ensured by using, eh, a very small amount of interpolation.
                # of course, we can do better, but who cares?
                x1, y1 = path[k-1]
                x2, y2 = path[k]
                x = (x1 + x2 * 1023) / 1024.
                y = (y1 + y2 * 1023) / 1024.

                # even if (x, y) is not in the path itself, it may coincide with some segment
                if all(not inside(p[i-1][0], p[i-1][1], x, y, p[i][0], p[i][1])
                       for i in xrange(len(p))): break
            else:
                assert False

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
        self.cmap = {} # index: glyph name
        self.remaps = {} # set name: a list of Remaps
        self.features = {} # feature name: a list of remap set names
        self.exclude_from_sample = set()

        if fp: self.read(fp)

    def read(self, fp):
        SubglyphArgs = namedtuple('SubglyphArgs', 'name placeholder roff coff filters adjoin')
        GlyphArgs = namedtuple('GlyphArgs', 'name flags lines parts pos2marks morepixels')

        CHAR_ITEM_PATTERN = re.compile(ur'''^(?:
            u\+(?P<start>[0-9a-f]{4,8})(?:\.\.(?P<end>[0-9a-f]{4,8})(?:/(?P<step>[0-9a-f]+))?)?
        )$''', re.I | re.X)
        def parse_char_name(s):
            c = try_ord(s)
            if c is not None: return [c]

            ret = []
            for i in s.split('|'):
                c = try_ord(i)
                if c is not None:
                    ret.append(c)
                else:
                    m = CHAR_ITEM_PATTERN.match(i)
                    if not m: raise ParseError(u'invalid character name/index %r' % s)

                    start = int(m.group('start'), 16)
                    end = m.group('end'); end = int(end, 16) if end is not None else None
                    step = m.group('step'); step = int(step, 16) if step is not None else 1
                    if end is not None:
                        ret += xrange(start, end + 1, step)
                    else:
                        assert step == 1
                        ret.append(start)

            return ret

        # glyph names are complex enough that name list can be replaced by
        # name parts reference `$<name>` or `($<name>)`.
        nameparts = {} # name including $: a list of strings

        GLYPH_NAME_PATTERN = re.compile(ur'''^(?:
            # single name
            (?P<name>[0-9a-z\-_.]+) |
            # name list
            (?P<names>[0-9a-z\-_.]*(?:\*\d+)? (?:\|[0-9a-z\-_.]*(?:\*\d+)?)* ) |
            # sole name parts reference
            (?P<ref>\$[0-9a-z\-_.]+) |
            # name list with one or more parts
            (?P<parts>[0-9a-z\-_.]*
                      (?: \( (?: \$[0-9a-z\-_.]+ |
                                 [0-9a-z\-_.]*(?:\*\d+)? (?:\|[0-9a-z\-_.]*(?:\*\d+)?)* ) \)
                          [0-9a-z\-_.]* )+ )
        )$''', re.I | re.X)
        def parse_glyph_name(s):
            m = GLYPH_NAME_PATTERN.match(s)
            if not m: raise ParseError(u'invalid glyph name %r' % s)

            if m.group('name'):
                return m.group('name').lower()

            # multiple names
            if m.group('names') or m.group('ref'): s = u'(%s)' % s # implicit parentheses
            parts = s.replace('(', ')').split(')')
            for i in xrange(1, len(parts), 2):
                if parts[i].startswith('$'):
                    try: parts[i] = nameparts[parts[i]]
                    except KeyError:
                        raise ParseError(u'invalid name parts reference %r' % parts[i])
                else:
                    part = []
                    for alt in parts[i].split('|'):
                        n, _, rep = alt.partition('*')
                        part.extend([n] * int(rep or 1))
                    parts[i] = part

            count = reduce(lcm, map(len, parts[1::2]), 1)
            ss = []
            for k in xrange(count):
                n = u''.join(x if i%2==0 else x[k%len(x)] for i, x in enumerate(parts))
                ss.append(n.lower())
            return ss

        def parse_subglyph_spec(s):
            filters = []
            adjoin = True
            if len(s) >= 3:
                s, _, filters = s.partition('!') # so that `filters` are always kept
                filters = map(unicode.lower, filter(None, filters.split('!')))
                for f in filters:
                    if f not in ('negate',):
                        raise ParseError(u'unrecognized filter name %r in '
                                         u'subglyph spec %r' % (f, s))

                name, _, placeholder = s.partition('@')
                if name:
                    if name.startswith('-'):
                        adjoin = False
                        name = name[1:]
                    if placeholder and all(c in '^v<>' for c in placeholder):
                        # displacement spec. e.g. ced@vv moves a cedilla one pixel down
                        roff = placeholder.count('v') - placeholder.count('^')
                        coff = placeholder.count('>') - placeholder.count('<')
                        return SubglyphArgs(name=parse_glyph_name(name), placeholder=None,
                                            roff=roff, coff=coff, filters=filters, adjoin=adjoin)
                    elif len(placeholder) == 1:
                        return SubglyphArgs(name=parse_glyph_name(name), placeholder=placeholder,
                                            roff=None, coff=None, filters=filters, adjoin=adjoin)

            return SubglyphArgs(name=parse_glyph_name(s), placeholder=None,
                                roff=None, coff=None, filters=filters, adjoin=adjoin)

        FILLED = frozenset(list('@b9Pd(u)n') + range(PX_FULL, (PX_FULL | PX_ALMOSTFULL) + 1))
        def parse_pixels(lines, bbox):
            width = len(lines[0])
            height = len(lines)
            pixels = []
            for rr, line in enumerate(lines):
                assert len(line) == width
                if not (bbox[0] <= rr <= bbox[2]): continue
                for cc, px in enumerate(line):
                    if not (bbox[1] <= cc <= bbox[3]): continue
                    if isinstance(px, int): # explicit pixel
                        pixels.append(px)
                        continue
                    v = None
                    t = lines[rr-1][cc]; b = lines[rr+1][cc]
                    l = lines[rr][cc-1]; r = lines[rr][cc+1]
                    if px == '@':
                        v = PX_ALMOSTFULL
                    elif px == '*':
                        # does not participate to the contextual positioning
                        v = PX_DOT | PX_FULL
                    elif px in '.!+':
                        v = PX_EMPTY
                    elif px == 'b':
                        # @@.@@
                        # @/d@@ <- avoid parsing this as connected
                        rcont = (r == '\\' and not (lines[rr][cc+2] in FILLED and
                                                    lines[rr-1][cc+1] in FILLED))
                        tcont = (t == '\\' and not (lines[rr-2][cc] in FILLED and
                                                    lines[rr-1][cc+1] in FILLED))
                        v = ((None           if rcont else PX_HALFSLANT1H) if tcont else
                             (PX_HALFSLANT1V if rcont else PX_HALF1))
                    elif px == '9':
                        lcont = (l == '\\' and not (lines[rr][cc-2] in FILLED and
                                                    lines[rr+1][cc-1] in FILLED))
                        bcont = (b == '\\' and not (lines[rr+2][cc] in FILLED and
                                                    lines[rr+1][cc-1] in FILLED))
                        v = ((None           if lcont else PX_HALFSLANT2H) if bcont else
                             (PX_HALFSLANT2V if lcont else PX_HALF2))
                    elif px == 'P':
                        rcont = (r == '/' and not (lines[rr][cc+2] in FILLED and
                                                   lines[rr+1][cc+1] in FILLED))
                        bcont = (b == '/' and not (lines[rr+2][cc] in FILLED and
                                                   lines[rr+1][cc+1] in FILLED))
                        v = ((None           if rcont else PX_HALFSLANT3H) if bcont else
                             (PX_HALFSLANT3V if rcont else PX_HALF3))
                    elif px == 'd':
                        lcont = (l == '/' and not (lines[rr][cc-2] in FILLED and
                                                   lines[rr-1][cc-1] in FILLED))
                        tcont = (t == '/' and not (lines[rr-2][cc] in FILLED and
                                                   lines[rr-1][cc-1] in FILLED))
                        v = ((None           if lcont else PX_HALFSLANT4H) if tcont else
                             (PX_HALFSLANT4V if lcont else PX_HALF4))
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

        # referenced by flush_glyph and define_glyph
        default_flags = 0
        default_left = 0
        default_width = None

        def flush_glyph(name, flags, lines, parts, pos2marks, morepixels):
            if not lines and not parts:
                raise ParseError(u'data for glyph %s is missing' % name)

            placeholder_bboxes = {}
            bbox = None
            morepixels.reverse() # so that .pop() works

            def update_bbox(prev, r, c):
                if not prev: return r, c, r, c
                return min(prev[0],r), min(prev[1],c), max(prev[2],r), max(prev[3],c)

            subglyphs = []
            points = {}
            if lines:
                # strip placeholders and out-of-bbox markers (+)
                placeholders = set(p.placeholder for p in parts if p.placeholder)
                newlines = []
                rowmarks = {}
                colmarks = {}
                r = 0 # should skip lines with mark =
                for line, rowmark in lines:
                    # a row with mark = is used to define column marks
                    if rowmark == '=':
                        for c, colmark in enumerate(line):
                            if colmark != '=':
                                if colmark in colmarks:
                                    raise ParseError(u'duplicate column mark %r in glyph %s' %
                                                     (colmark, name))
                                colmarks[colmark] = c
                    else:
                        if rowmark:
                            if rowmark in rowmarks:
                                raise ParseError(u'duplicate row mark %r in glyph %s' %
                                                 (rowmark, name))
                            rowmarks[rowmark] = r
                        newline = []
                        for c, px in enumerate(line):
                            if px == '?':
                                try:
                                    px = morepixels.pop()
                                except IndexError:
                                    raise ParseError(u'not enough explicit pixels for glyph %s' %
                                                     name)
                            elif px in placeholders:
                                placeholder_bboxes[px] = \
                                        update_bbox(placeholder_bboxes.get(px), r, c)
                                px = '!'
                            if px != '+':
                                bbox = update_bbox(bbox, r, c)
                            newline.append(px)
                        newline.append('.')
                        newlines.append(newline)
                        r += 1
                newlines.append(['.'] * len(newlines[0]))
                lines = newlines
                if morepixels:
                    raise ParseError(u'too many explicit pixels for glyph %s' % name)

                if not bbox:
                    raise ParseError(u'glyph for %s is empty' % name)
                try:
                    pixels = parse_pixels(lines, bbox)
                except ParseError as e:
                    raise ParseError(u'parsing glyph for %s failed: %s' % (name, e))

                # resolve named positions
                for posname, mark in pos2marks.items():
                    assert 1 <= len(mark) <= 2
                    try:
                        pos1 = rowmarks[mark[0]], colmarks[mark[1:] or mark]
                    except KeyError:
                        pos1 = None
                    try:
                        pos2 = rowmarks[mark[1:] or mark], colmarks[mark[0]]
                    except KeyError:
                        pos2 = None
                    if pos1 and pos2 and len(mark) == 2:
                        raise ParseError(u'ambiguous mark %r for position name %r in glyph %s' %
                                         (mark, posname, name))
                    elif pos1 or pos2:
                        r, c = pos1 or pos2
                        points[posname] = r - bbox[0], c - bbox[1]
                    else:
                        raise ParseError(u'missing mark %r for position name %r in glyph %s' %
                                         (mark, posname, name))

                # this may be empty if entire glyph consists of subglyphs.
                top, left, bottom, right = bbox
                height = bottom - top + 1
                width = right - left + 1
                subglyphs.append(Subglyph(top=0, left=default_left, height=height, width=width,
                                          stride=width, data=pixels, negated=0))

            # we now have proper relocation infos for subglyphs if any
            roff = bbox[0] if bbox else 0
            coff = bbox[1] if bbox else 0
            for p in parts:
                negated = 1 if 'negate' in p.filters else 0
                subname = Adjoin(value=p.name) if p.adjoin else p.name
                if p.placeholder in placeholder_bboxes:
                    subtop, subleft, subbottom, subright = placeholder_bboxes[p.placeholder]
                    subheight = subbottom - subtop + 1
                    subwidth = subright - subleft + 1
                    subglyphs.append(Subglyph(top=subtop-roff+(p.roff or 0),
                                              left=subleft-coff+(p.coff or 0)+default_left,
                                              width=subwidth, height=subheight,
                                              stride=None, data=subname, negated=negated))
                else:
                    subglyphs.append(Subglyph(top=None if p.roff is None else Delta(p.roff),
                                              left=None if p.coff is None else Delta(p.coff),
                                              width=None, height=None,
                                              stride=None, data=subname, negated=negated))

            if name == u'.notdef': flags |= G_STICKY
            # if default_width is None, advance is calculated from intrinsic width later
            self.glyphs[name] = Glyph(flags=flags, height=None, width=default_width,
                                      preferred_top=roff, preferred_left=coff,
                                      subglyphs=subglyphs, points=points)

        def define_glyph(name, specs):
            if isinstance(name, basestring):
                name = parse_glyph_name(name)
            if isinstance(name, basestring):
                # single glyph definition
                if name in self.glyphs:
                    raise ParseError(u'duplicate glyph %s' % name)
                subglyph_spec = map(parse_subglyph_spec, specs)
                for p in subglyph_spec:
                    if not isinstance(p.name, basestring):
                        raise ParseError(u'invalid use of multiple subglyphs in %s' % name)
                placeholder_chars = [p.placeholder for p in subglyph_spec if p.placeholder]
                if len(placeholder_chars) != len(set(placeholder_chars)):
                    raise ParseError(u'duplicate placeholder characters for glyph %s' % name)
                return GlyphArgs(name=name, flags=default_flags, lines=[], parts=subglyph_spec,
                                 pos2marks={}, morepixels=[])
            else:
                # multiple glyph definition (combined glyph only)
                names = name
                for name in names:
                    if name in self.glyphs:
                        raise ParseError(u'duplicate glyph %s' % name)
                subglyph_spec = map(parse_subglyph_spec, specs)
                placeholder_chars = [p.placeholder for p in subglyph_spec if p.placeholder]
                if len(placeholder_chars) != len(set(placeholder_chars)):
                    raise ParseError(u'duplicate placeholder characters for glyph %s' % args[1])
                for j, p in enumerate(subglyph_spec):
                    if isinstance(p.name, basestring):
                        subnames = itertools.repeat(p.name)
                    else:
                        subnames = itertools.cycle(p.name)
                    subglyph_spec[j] = p._replace(name=subnames)
                for i, name in enumerate(names):
                    spec = [p._replace(name=p.name.next()) for p in subglyph_spec]
                    flush_glyph(name, default_flags, [], spec, {}, [])
                return None

        current_glyph = None # or GlyphArgs
        prev_args = []
        TOKEN_PATTERN = re.compile(ur'`(?:[^`]|``)*`|\S+')
        for line in fp:
            args = []
            for tok in TOKEN_PATTERN.findall(line.decode('utf-8')):
                if tok.startswith('//'): break # beginning of comment
                if tok.startswith('`'):
                    assert len(tok) >= 2 and tok.endswith('`')
                    args.append(tok[1:-1].replace('``', '`'))
                elif '`' in tok:
                    raise ParseError('unquoted backquotes in token %r' % tok)
                else:
                    args.append(tok)
            if not args: continue
            if args[-1] == '..': # continuation token
                prev_args.extend(args[:-1])
                continue
            else:
                args = prev_args + args
                prev_args = []

            if args[0] == 'glyph':
                # glyph <glyph>
                # glyph <glyph> = <subglyph> ... <subglyph>
                if current_glyph:
                    flush_glyph(*current_glyph)
                    current_glyph = None
                if len(args) > 3 and args[2] == '=': # combined glyph
                    current_glyph = define_glyph(args[1], args[3:])
                elif len(args) == 2:
                    name = parse_glyph_name(args[1])
                    if not isinstance(name, basestring):
                        raise ParseError(u'unexpected arguments to `glyph`')
                    current_glyph = GlyphArgs(name=name, flags=default_flags, lines=[], parts=[],
                                              pos2marks={}, morepixels=[])
                else:
                    raise ParseError(u'unexpected arguments to `glyph`')

            elif args[0] == 'map':
                # map <char> = <glyph>
                # map <char> = <subglyph> ... <subglyph>
                if len(args) <= 3 or args[2] != '=':
                    raise ParseError(u'unexpected arguments to `map`')

                if current_glyph:
                    flush_glyph(*current_glyph)
                    current_glyph = None
                chars = parse_char_name(args[1])
                glyphs = None
                if len(args) == 4:
                    try: glyphs = parse_glyph_name(args[3])
                    except Exception: pass
                if glyphs is None:
                    # implicit glyph definition precedes
                    glyphs = ['uni%04X' % ch for ch in chars]
                    glyph_spec = define_glyph(glyphs, args[3:])
                    if glyph_spec: flush_glyph(*glyph_spec) # possible when len(chars) == 1
                elif isinstance(glyphs, basestring):
                    glyphs = [glyphs]
                for ch, glyph in zip(chars, itertools.cycle(glyphs)):
                    if ch in self.cmap:
                        raise ParseError(u'duplicate character %s' % char_name(ch))
                    self.cmap[ch] = glyph

            elif args[0] == 'remap':
                # remap <set> : [<lookbehind glyph> ... :] <glyph> ... -> <glyph> ...
                #               [: <lookahead glyph> ...]
                #
                # it should be noted that while remap can be defined for multiple glyphs
                # there is a strong restriction for each rule: both input and output
                # glyphs can contain at most *one* glyph containing parentheses,
                # and when both input and output glyphs contain such glyph name,
                # the number of generated glyphs should match (and applied pairwise).
                # lookbehind and lookahead glyphs are distinct and may contain
                # any number of parentheses (as it is essentially a matching pattern).
                # this restriction makes a straight mapping to GSUB substitution rules.
                #
                # e.g. (a|b) (c|d|e) : (f|g) -> (h|i) : (j|k|l|m) is valid;
                #      the substitution is `f -> h` and `g -> i`,
                #      the lookbehind is `a c`, `a d`, `a e`, `b c`, `b d` or `b e`,
                #      the lookahead is `j`, `k`, `l` or `m`.

                if len(args) <= 2 or args[2] != ':':
                    raise ParseError(u'unexpected arguments to `remap`')

                setname = args[1]
                lookbehind = []
                pattern = []
                replacement = []
                lookahead = []
                aftercolon = afterarrow = False
                current = []
                for arg in args[3:]:
                    if arg == ':':
                        if aftercolon: raise ParseError(u'too many `:` in `remap` arguments')
                        if afterarrow: replacement = current # ... : (here) ... -> ... : ...
                        else:           lookbehind = current # ... : ... -> ... : (here) ...
                        current = []
                        aftercolon = True
                    elif arg == '->':
                        if afterarrow: raise ParseError(u'too many `->` in `remap` arguments')
                        pattern = current # ... : ... -> (here) ... : ...
                        current = []
                        afterarrow = True
                        aftercolon = False
                    else:
                        current.append(parse_glyph_name(arg))
                if not afterarrow: raise ParseError(u'missing `->` in `remap` arguments')
                if aftercolon: lookahead = current # ... : ... -> ... : ... (here)
                else:        replacement = current # ... : ... -> ... (here)

                if not pattern or not replacement:
                    raise ParseError(u'missing pattern or replacement in `remap` arguments')

                # quick check, so we don't have to repeat ourselves in render
                patcount = None
                repcount = None
                for pat in pattern:
                    if not isinstance(pat, list): continue
                    if patcount is not None:
                        raise ParseError(u'`remap` pattern has multiple parentheses')
                    patcount = len(pat)
                for rep in replacement:
                    if not isinstance(rep, list): continue
                    if repcount is not None:
                        raise ParseError(u'`remap` replacement has multiple parentheses')
                    repcount = len(rep)
                if patcount is None and repcount is not None:
                    raise ParseError(u'`remap` cannot map a single input to multiple outputs '
                                     u'with `->` operator')
                if patcount is not None and repcount is not None and patcount != repcount:
                    raise ParseError(u'`remap` pattern and replacement both has parentheses '
                                     u'but the number of sequences does not match')

                self.remaps.setdefault(setname, []).append(Remap(
                    lookbehind=lookbehind, pattern=pattern,
                    lookahead=lookahead, replacement=replacement))

            elif args[0] == 'feature':
                # feature <feature> : <set> ... [: <condition> ...]
                if len(args) <= 2 or args[2] != ':':
                    raise ParseError(u'unexpected arguments to `feature`')

                featurename = args[1]
                if len(featurename) != 4:
                    raise ParseError(u'invalid feature name %r' % featurename)

                sets = self.features.setdefault(featurename, [])
                for arg in args[3:]:
                    if arg == ':':
                        raise ParseError(u'conditional features are not yet supported')
                    sets.append(arg)

            elif args[0] == 'exclude-from-sample':
                # exclude-from-sample <char> ...
                for arg in args[1:]:
                    for name in parse_char_name(arg):
                        self.exclude_from_sample.add(name)

            elif args[0] == 'default':
                # default +<option> [<args> ...]
                # default -<option>
                if current_glyph:
                    flush_glyph(*current_glyph)
                    current_glyph = None

                params = args[1:]
                if not params:
                    raise ParseError(u'missing `default` arguments')
                elif params == ['+sticky']:
                    default_flags |= G_STICKY
                elif params == ['-sticky']:
                    default_flags &= ~G_STICKY
                elif params[0] == '+left' and len(params) == 2:
                    try: default_left = int(params[1])
                    except ValueError:
                        raise ParseError(u'`default +left` should be an integer')
                elif params == ['-left']:
                    default_left = 0
                elif params[0] == '+advance' and len(params) == 2:
                    try: default_width = int(params[1])
                    except ValueError:
                        raise ParseError(u'`default +advance` should be an integer')
                elif params == ['-advance']:
                    default_width = None
                else:
                    raise ParseError(u'unknown `default` arguments %r' % params)

            elif args[0] == 'name-parts':
                # name-parts $<name> = <str> <str>|<str> <str>*<count> $<ref> ...
                # | is exactly identical to whitespace for the purpose of this command.
                if len(args) <= 3 or args[2] != '=':
                    raise ParseError(u'unexpected arguments to `name-parts`')

                partsname = args[1]
                if not re.search(r'(?i)^\$[0-9a-z\-_.]+$', partsname):
                    raise ParseError(u'invalid name %r for `name-parts`' % partsname)
                if partsname in nameparts:
                    raise ParseError(u'duplicate name %r for `name-parts`' % partsname)

                partslist = []
                for parts in args[3:]:
                    if re.search(r'(?i)^\$[0-9a-z\-_.]+$', parts):
                        try: partslist.extend(nameparts[parts])
                        except KeyError:
                            raise ParseError(u'invalid name parts reference %r in name parts %s' %
                                             (parts, partsname))
                    else:
                        for part in parts.split('|'):
                            part, _, rep = part.partition('*')
                            if not re.search(r'(?i)^[0-9a-z\-_.]*$', part): # may be empty!
                                raise ParseError(u'name parts %s has an invalid part %r' %
                                                 (partsname, part))
                            if rep and not rep.isdigit():
                                raise ParseError(u'non-integral repeating count '
                                                 u'in name parts %s' % partsname)
                            partslist.extend([part] * int(rep or 1))
                nameparts[partsname] = partslist

            else:
                if not current_glyph:
                    raise ParseError(u'no glyph currently active')

                # glyph definition and command may go in the same line
                retrying = False
                while args:
                    first = args.pop(0)

                    if first == 'inline':
                        current_glyph = current_glyph._replace(flags=current_glyph.flags | G_INLINE)
                        break

                    elif first == 'sticky':
                        current_glyph = current_glyph._replace(flags=current_glyph.flags | G_STICKY)
                        break

                    elif first == 'point':
                        # point <point>@<mark> ...
                        for pointspec in args:
                            posname, _, mark = pointspec.partition('@')
                            posname = posname.lower()
                            if not (posname and 1 <= len(mark) <= 2):
                                raise ParseError(u'invalid point specification %r in glyph %s' %
                                                 (pointspec, current_glyph.name))
                            if posname in current_glyph.pos2marks:
                                raise ParseError(u'duplicate position name %r in glyph %s' %
                                                 (posname, current_glyph.name))
                            current_glyph.pos2marks[posname] = mark
                        break

                    elif first == 'pixel':
                        # pixel <pixelspec> ...
                        for pixelspec in args:
                            try:
                                current_glyph.morepixels.append(PIXELNAMES[pixelspec])
                            except KeyError:
                                raise ParseError(u'invalid explicit pixel specification %r '
                                                 u'in glyph %s' % (pixelspec, current_glyph.name))
                        break

                    elif retrying:
                        raise ParseError(u'unrecogized command %r after definition in glyph %s' %
                                         (first, current_glyph.name))

                    else:
                        if current_glyph.lines and len(current_glyph.lines[0][0]) != len(first):
                            raise ParseError(u'inconsistent glyph width for %s' %
                                             current_glyph.name)

                        if args and len(args[0]) == 1:
                            rowmark = args.pop(0)
                        else:
                            rowmark = None
                        current_glyph.lines.append((first, rowmark))
                        retrying = True

        if current_glyph: flush_glyph(*current_glyph)

    def resolve_glyphs(self):
        resolved = set()
        def resolve(name):
            if name in resolved: return
            resolved.add(name)

            gg = self.glyphs[name]
            assert isinstance(gg.preferred_top, int)
            assert isinstance(gg.preferred_left, int)

            maxbottom = 0
            maxright = 0
            for k, g in enumerate(gg.subglyphs):
                top, left, height, width, stride, data, negated = g
                if isinstance(data, list):
                    # every other field should be final
                    assert isinstance(top, int)
                    assert isinstance(left, int)
                    assert isinstance(height, int)
                    assert isinstance(width, int)
                    assert isinstance(stride, int)
                else:
                    adjoin = False
                    if isinstance(data, Adjoin):
                        adjoin = True
                        data = data.value

                    gg2 = self.glyphs[data]

                    # if gg2 doesn't have enough info to resolve g's fields, try to resolve
                    # (for gg2.height and gg2.height, we don't strictly need that to resolve,
                    # but we use them for sanity checking so request them this early)
                    if not ((isinstance(top, int) or gg2.preferred_top is not None) and
                            (isinstance(left, int) or gg2.preferred_left is not None) and
                            (isinstance(height, int) and gg2.height is not None) and
                            (isinstance(width, int) and gg2.width is not None)):
                        resolve(data)
                        gg2 = self.glyphs[data] # since gg2 is, eh, an immutable tuple

                    # if the resolution has failed, it has a cyclic dependency
                    if not ((isinstance(top, int) or gg2.preferred_top is not None) and
                            (isinstance(left, int) or gg2.preferred_left is not None) and
                            (isinstance(height, int) or gg2.height is not None) and
                            (isinstance(width, int) or gg2.width is not None)):
                        raise ParseError(u'glyph %s has a cyclic dependency' % name)

                    # resolve the default joining position if available
                    joining_delta = None
                    newpoints = set()
                    if adjoin:
                        for posname, (pr, pc) in gg2.points.items():
                            if posname.startswith('-'):
                                posname2 = '+' + posname[1:]
                                if posname2 in gg.points: # adjoin
                                    qr, qc = gg.points.pop(posname2)
                                    delta = qr - pr, qc - pc
                                    if joining_delta and joining_delta != delta:
                                        raise ParseError(u'glyph %s has multiple adjoining points '
                                                         u'inconsistent to each other' % name)
                                    joining_delta = delta
                                else:
                                    newpoints.add(posname)
                            elif posname.startswith('+'):
                                # the parent glyph may override the same point
                                if posname not in gg.points:
                                    newpoints.add(posname)
                    preferred_top, preferred_left = \
                            joining_delta or (gg2.preferred_top, gg2.preferred_left)

                    if top is None: top = preferred_top
                    elif isinstance(top, Delta): top = preferred_top + top.value
                    if left is None: left = preferred_left
                    elif isinstance(left, Delta): left = preferred_left + left.value
                    if height is None: height = gg2.height
                    if width is None: width = gg2.width

                    if (width, height) != (gg2.width, gg2.height):
                        raise ParseError(u'glyph %s has a scaled subglyph for %s '
                                         u'that is unsupported: requested %sx%s, actual %sx%s' %
                                         (name, data, width, height, gg2.width, gg2.height))

                    for posname in newpoints:
                        r, c = gg2.points[posname]
                        gg.points[posname] = (r - gg2.preferred_top + preferred_top,
                                              c - gg2.preferred_left + preferred_left)

                    gg.subglyphs[k] = Subglyph(top=top, left=left,
                                               height=height, width=width,
                                               stride=stride, data=data, negated=negated)

                maxbottom = max(maxbottom, top + height)
                maxright = max(maxright, left + width)

            self.glyphs[name] = gg._replace(
                height=maxbottom if gg.height is None else gg.height,
                width=maxright if gg.width is None else gg.width)

        for name in self.glyphs.keys(): resolve(name)

        def make_sticky(name):
            try:
                gg = self.glyphs[name]
                self.glyphs[name] = gg._replace(flags=gg.flags | G_STICKY)
                return True
            except KeyError:
                return False

        for ch, name in self.cmap.items():
            if not make_sticky(name):
                raise ParseError(u'character %s is mapped to non-existant glyph %s' %
                                 (char_name(ch), name))

        for setname, remaps in self.remaps.items():
            for remap in remaps:
                for names in (remap.lookbehind, remap.pattern, remap.replacement, remap.lookahead):
                    for name in names:
                        if not isinstance(name, list): name = [name]
                        for iname in name:
                            if not make_sticky(iname):
                                raise ParseError(u'remapping set %s has a reference to '
                                                 u'non-existant glyph %s' % (setname, iname))

        for featurename, sets in self.features.items():
            # deduplicate the set names
            seen = set()
            offset = 0
            for i in xrange(len(sets)):
                if sets[i] in seen:
                    offset += 1
                elif sets[i] not in self.remaps:
                    raise ParseError(u'feature %s has a reference to non-existant '
                                     u'remapping set %s' % (featurename, sets[i]))
                else:
                    sets[i-offset] = sets[i]
            if offset: del sets[-offset:]

    def inline_glyphs(self):
        # invariant: redirect_to[a][0] == b <=> a in redirect_from[b]
        empty = set()
        redirect_to = {} # name: None or (subname, roff, coff)
        redirect_from = {} # subname: set of names
        def check(name):
            if name in redirect_to: return
            redirect_to[name] = None

            gg = self.glyphs[name]

            # eliminate empty subglyphs (while recursively checking others)
            subglyphs = []
            for g in gg.subglyphs:
                if isinstance(g.data, basestring):
                    check(g.data)
                    if g.data not in empty: subglyphs.append(g)
                else:
                    if any(g.data): subglyphs.append(g)
            gg.subglyphs[:] = subglyphs

            if not subglyphs:
                if not (gg.flags & G_STICKY): empty.add(name)
                return
            elif len(subglyphs) != 1:
                return

            # mark as redirected if this glyph consists of a single component
            g = subglyphs[0]
            if not isinstance(g.data, basestring): return
            if g.negated != 0: return # XXX
            subname, roff, coff = redirect_to[g.data] or (g.data, 0, 0)
            roff += g.top
            coff += g.left
            if gg.flags & G_STICKY:
                # A (sticky) --[off1]--> a-upper --[off2]--> a-upper-aux
                #              swapped             inlined
                #
                # A (sticky) <-----------[-off1-off2]------------+ inlined
                #                                  inlined       |
                #                        a-upper --[off2]--> a-upper-aux
                #
                # we still need to avoid swapping if a-upper is replaced by, say, cyrillic А
                gg2 = self.glyphs[subname]
                if gg2.flags & G_STICKY: return
                gg.subglyphs[:] = [g2._replace(top=g2.top+roff, left=g2.left+coff)
                                   for g2 in gg2.subglyphs]
                for iname in redirect_from.get(subname, ()): # re-inlining
                    isubname, iroff, icoff = redirect_to[iname]
                    assert subname == isubname
                    redirect_to[iname] = name, iroff - roff, icoff - coff
                redirect_to[subname] = name, -roff, -coff
                redirect_from.setdefault(name, set()).update(redirect_from.pop(subname, ()))
                del self.glyphs[subname]
            else:
                redirect_to[name] = subname, roff, coff
                redirect_from.setdefault(subname, set()).add(name)
                del self.glyphs[name]

        for name in self.glyphs.keys(): check(name)

        for name, gg in self.glyphs.items():
            if name in empty:
                del self.glyphs[name]
            else:
                subglyphs = gg.subglyphs
                subglyphs.reverse() # so that we can pop the next subglyph easily
                newsubglyphs = []
                while subglyphs:
                    g = subglyphs.pop()
                    if isinstance(g.data, basestring):
                        if redirect_to[g.data]:
                            subname, roff, coff = redirect_to[g.data]
                            newsubglyphs.append(g._replace(top=g.top+roff, left=g.left+coff,
                                                           data=subname))
                            continue
                        gg2 = self.glyphs[g.data]
                        if gg2.flags & G_INLINE:
                            for g2 in reversed(gg2.subglyphs):
                                subglyphs.append(g2._replace(top=g2.top+g.top,
                                                             left=g2.left+g.left,
                                                             negated=g2.negated+g.negated))
                            continue
                    newsubglyphs.append(g)
                subglyphs[:] = newsubglyphs

        # inlined glyphs no longer require to be included
        for name, gg in self.glyphs.items():
            if gg.flags & G_INLINE: del self.glyphs[name]

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
        glyphids = {}
        hasnotdef = False
        def custom_sort_key((name, _)):
            # what, the, real, fuck.
            # it seems that Uniscribe has some bug with Hangul and possibly more scripts:
            # some characters, when they are located in specific glyph indices, are correctly
            # mapped via ScriptGetCMap but considered to be missing via ScriptShape.
            # combined with SSA_FALLBACK it causes the wrong *and* inconsistent fallback behavior.
            # given that the range of those indices abruptly end with 2^n boundaries,
            # I strongly suspect that this is something to do with the internal lookup mechanism.
            # for now, reorder problematic scripts to (empirically) avoid the problem... *sigh*
            if not name.startswith('uni'): return (2, name)
            try: c = int(name[3:], 16)
            except ValueError: return (2, name)
            return (0 if 0x1100 <= c <= 0x11ff or 0x3130 <= c <= 0x318f or
                         0xa960 <= c <= 0xa97f or 0xac00 <= c <= 0xd7ff else 1, c)
        for name, gg in sorted(self.glyphs.items(), key=custom_sort_key):
            if name == '.notdef': hasnotdef = True
            compositecount = sum(isinstance(g.data, basestring) for g in gg.subglyphs)
            if 0 < compositecount < len(gg.subglyphs):
                # add intermediate subglyphs when required
                for i, g in enumerate(gg.subglyphs):
                    if not isinstance(g.data, list): continue
                    subnames.append(('%s#%d' % (name, i), g.width,
                                     get_lsb_from_pixels(g.height, g.width, g.stride, g.data)))
            subnames.append((name, gg.width, get_lsb(name)))
            glyphids[name] = len(glyphids)
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
        print >>fp, '<minLeftSideBearing value="0"/>'
        print >>fp, '<minRightSideBearing value="0"/>'
        print >>fp, '<xMaxExtent value="0"/>'
        print >>fp, '</hhea>'

        # head
        timestamp = time.ctime() # XXX
        print >>fp, '<head>'
        print >>fp, '<magicNumber value="0x5f0f3cf5"/>'
        print >>fp, '<fontRevision value="1.0"/>'
        print >>fp, '<unitsPerEm value="{emsize}"/>'.format(emsize=emsize)
        print >>fp, '<created value="{created}"/>'.format(created=timestamp)
        print >>fp, '<lowestRecPPEM value="8"/>'
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<tableVersion value="1.0"/>'
        print >>fp, '<checkSumAdjustment value="0"/>'
        print >>fp, '<flags value="00000000 00001011"/>'
        print >>fp, '<modified value="{modified}"/>'.format(modified=timestamp)
        print >>fp, '<xMin value="0"/>'
        print >>fp, '<yMin value="0"/>'
        print >>fp, '<xMax value="0"/>'
        print >>fp, '<yMax value="0"/>'
        print >>fp, '<macStyle value="00000000 00000000"/>'
        print >>fp, '<fontDirectionHint value="2"/>'
        print >>fp, '<indexToLocFormat value="1"/>'
        print >>fp, '<glyphDataFormat value="0"/>'
        print >>fp, '</head>'

        # maxp
        print >>fp, '<maxp>'
        print >>fp, '<tableVersion value="0x10000"/>'
        print >>fp, '<maxZones value="2"/>'
        print >>fp, '<maxFunctionDefs value="40"/>'
        print >>fp, '<maxInstructionDefs value="0"/>'
        print >>fp, '<maxStackElements value="512"/>'
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<numGlyphs value="0"/>'
        print >>fp, '<maxPoints value="0"/>'
        print >>fp, '<maxContours value="0"/>'
        print >>fp, '<maxCompositePoints value="0"/>'
        print >>fp, '<maxCompositeContours value="0"/>'
        print >>fp, '<maxTwilightPoints value="0"/>'
        print >>fp, '<maxStorage value="0"/>'
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
        print >>fp, '<fsType value="00000000 00000000"/>' # no further embedding restrictions
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
        # http://forum.high-logic.com/postedfiles/Panose.pdf
        print >>fp, '  <bFamilyType value="2"/>' # latin text
        print >>fp, '  <bSerifStyle value="11"/>' # normal sans
        print >>fp, '  <bWeight value="6"/>' # medium
        print >>fp, '  <bProportion value="9"/>' # monospaced (important!)
        print >>fp, '  <bContrast value="1"/>' # well, frankly I don't care others
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
        exbmp = False
        for platid, platenc in ((0, 3), (1, 0), (3, 1)):
            print >>fp, '<cmap_format_4 platformID="{platid}" platEncID="{platenc}" ' \
                         'language="0">'.format(platid=platid, platenc=platenc)
            for ch, name in sorted(self.cmap.items()):
                if ch >= 0x10000: exbmp = True; continue
                print >>fp, '<map code="{ch:#x}" name="{name}"/>'.format(ch=ch, name=escape(name))
            print >>fp, '</cmap_format_4>'
        if exbmp:
            for platid, platenc in ((0, 4), (3, 10)):
                print >>fp, '<!-- length/nGroups fields are automatically updated: -->'
                print >>fp, '<cmap_format_12 platformID="{platid}" platEncID="{platenc}" ' \
                             'language="0" format="12" reserved="0" length="0" ' \
                             'nGroups="0">'.format(platid=platid, platenc=platenc)
                for ch, name in sorted(self.cmap.items()):
                    print >>fp, '<map code="{ch:#x}" name="{name}"/>'.format(ch=ch,
                                                                             name=escape(name))
                print >>fp, '</cmap_format_12>'
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
        def flush_contour(fp, g, dx, dy):
            assert isinstance(g.data, list)
            for contour in track_contour(g.height, g.width, g.stride, g.data, PX_SUBPIXEL):
                print >>fp, '<contour>'
                for x, y in contour:
                    x = int(SCALE * (dx + x))
                    y = int(SCALE * (dy + (g.height - y)))
                    print >>fp, '<pt x="{x}" y="{y}" on="1"/>'.format(x=x, y=y)
                print >>fp, '</contour>'
        for name, gg in sorted(self.glyphs.items()):
            name = get_subname(name)

            compositecount = sum(isinstance(g.data, basestring) for g in gg.subglyphs)
            hybrid = (0 < compositecount < len(gg.subglyphs))
            if hybrid:
                for i, g in enumerate(gg.subglyphs):
                    if not isinstance(g.data, list): continue
                    subname = '%s#%d' % (name, i)
                    print >>fp, '<TTGlyph name="{subname}">'.format(subname=subname)
                    flush_contour(fp, g, 0, 0)
                    print >>fp, '<instructions><bytecode></bytecode></instructions>'
                    print >>fp, '</TTGlyph>'

            print >>fp, '<TTGlyph name="{name}">'.format(name=name)
            for i, g in enumerate(gg.subglyphs):
                if isinstance(g.data, list):
                    subname = '%s#%d' % (name, i)
                    subheight = g.height
                else:
                    subname = get_subname(g.data)
                    subheight = self.glyphs[g.data].height # NOT g.height, which can be wrong
                x = g.left
                y = gg.height - (g.top + subheight)
                if not hybrid and isinstance(g.data, list):
                    flush_contour(fp, g, x, y)
                else:
                    print >>fp, '<component glyphName="{subname}" x="{x}" y="{y}" ' \
                                 'flags="0x1004"/>'.format(subname=subname,
                                                           x=int(x*SCALE), y=int(y*SCALE))
            print >>fp, '<instructions><bytecode></bytecode></instructions>'
            print >>fp, '</TTGlyph>'
        print >>fp, '</glyf>'

        # name
        print >>fp, '<name>'
        names = [
            (0, 'copyright', u'Made by Kang Seonghoon; released in the public domain.'),
            (1, 'family', u'Unison'),
            (2, 'subfamily', u'Regular'),
            (3, 'identifier', u'Unison'),
            (4, 'fontname', u'Unison'),
            (5, 'version', u'Version 0.1'),
            (6, 'psname', u'Unison'),
            (13, 'license', u'Public Domain. Or alternatively, CC0 1.0 Universal.'),
        ]
        for platid, platenc, langid in ((1, 0, 0), (3, 1, 0x409)):
            for nameid, _, nameval in names:
                print >>fp, '<namerecord nameID="{id}" platformID="{platid}" ' \
                             'platEncID="{platenc}" langID="{langid}" ' \
                             'unicode="True">{val}</namerecord>'.format(
                                     id=nameid, platid=platid, platenc=platenc, langid=langid,
                                     val=nameval.encode('utf-8'))
        print >>fp, '</name>'

        # post
        print >>fp, '<post>'
        print >>fp, '<formatType value="2.0"/>'
        print >>fp, '<italicAngle value="0.0"/>'
        print >>fp, '<underlinePosition value="{ulinepos}"/>'.format(ulinepos=descent)
        print >>fp, '<underlineThickness value="{ulinesize}"/>'.format(ulinesize=int(SCALE))
        print >>fp, '<isFixedPitch value="1"/>' # monospaced
        print >>fp, '<!-- automatically updated: -->'
        print >>fp, '<minMemType42 value="0"/>'
        print >>fp, '<maxMemType42 value="0"/>'
        print >>fp, '<minMemType1 value="0"/>'
        print >>fp, '<maxMemType1 value="0"/>'
        print >>fp, '<psNames/>'
        print >>fp, '<extraNames/>'
        print >>fp, '</post>'

        # OpenType features
        lookups = []
        settolookup = {}
        for setname, remaps in self.remaps.items():
            lines = []

            # determine the most compact format for given remaps
            if all(len(r.pattern) == 1 and len(r.replacement) == 1 and
                   len(r.lookbehind) == 0 and len(r.lookahead) == 0 for r in remaps):
                # single substitution: a -> b
                lines.append('<LookupFlag value="0"/>')
                lines.append('<SingleSubst index="0">')
                for r in remaps:
                    pattern, = r.pattern
                    replacement, = r.replacement
                    if not isinstance(pattern, list): pattern = [pattern]
                    if not isinstance(replacement, list): replacement = [replacement]
                    for pat, rep in zip(pattern, itertools.cycle(replacement)):
                        lines.append('<Substitution in="{pat}" out="{rep}"/>'.format(pat=pat,
                                                                                     rep=rep))
                lines.append('</SingleSubst>')

            elif all(len(r.pattern) > 1 and len(r.replacement) == 1 and
                     len(r.lookbehind) == 0 and len(r.lookahead) == 0 for r in remaps):
                # ligature substitution: a1 a2 a3 -> b
                starts = {}
                for r in remaps:
                    patterns = r.pattern
                    replacement, = r.replacement
                    patterns = list(itertools.product(*[x if isinstance(x, list) else [x]
                                                        for x in patterns]))
                    if not isinstance(replacement, list): replacement = [replacement]
                    if all(len(pat) == 1 for pat in patterns):
                        assert len(replacement) == 1
                        patterns = [pat for pat, in patterns]
                        starts.setdefault(pat[0], []).append((pat[1:], replacement[0]))
                    else:
                        if len(replacement) == 1:
                            replacement = itertools.repeat(replacement[0], len(patterns))
                        else:
                            assert len(replacement) == len(patterns)
                        for pat, rep in zip(patterns, replacement):
                            starts.setdefault(pat[0], []).append((pat[1:], rep))
                lines.append('<LookupFlag value="0"/>')
                lines.append('<LigatureSubst index="0">')
                for start, ligatures in starts.items():
                    ligatures.sort(key=lambda (k,v): (-len(k), k, v))
                    lines.append('<LigatureSet glyph="{start}">'.format(start=start))
                    for remainder, mapped in ligatures:
                        lines.append('<Ligature components="{remainder}" glyph="{mapped}"/>'.format(
                            remainder=','.join(remainder), mapped=mapped))
                    lines.append('</LigatureSet>')
                lines.append('</LigatureSubst>')

            elif all(len(r.pattern) == 1 and len(r.replacement) == 1 for r in remaps):
                # chaining contextual substitution: lb1 lb2 : a -> b : la1 la2
                # unlike single/ligature substs, we keep each `r` in `remaps`.
                lines = []
                lines.append('<LookupFlag value="0"/>')
                for i, r in enumerate(remaps):
                    # we need a separate lookup to be run when the context matches
                    chainedlines = []
                    chainedlines.append('<LookupFlag value="0"/>')
                    chainedlines.append('<SingleSubst index="0">')
                    pats, = r.pattern
                    reps, = r.replacement
                    if not isinstance(pats, list): pats = [pats]
                    if not isinstance(reps, list): reps = [reps]
                    if len(pats) > len(reps):
                        assert len(pats) % len(reps) == 0
                        reps *= len(pats) // len(reps)
                    for pat, rep in zip(pats, reps):
                        chainedlines.append('<Substitution in="{pat}" '
                                                          'out="{rep}"/>'.format(pat=pat, rep=rep))
                    chainedlines.append('</SingleSubst>')
                    chainedlookup = len(lookups)
                    chainedsetname = '%s#%d' % (setname, i)
                    lookups.append((chainedsetname, chainedlines))

                    lines.append('<ChainContextSubst index="{index}" Format="3">'.format(index=i))
                    for j, glyphs in enumerate(reversed(r.lookbehind)):
                        if not isinstance(glyphs, list): glyphs = [glyphs]
                        lines.append('<BacktrackCoverage index="{index}">'.format(index=j))
                        for glyph in sorted(glyphs, key=lambda name: glyphids[name]):
                            lines.append('<Glyph value="{glyph}"/>'.format(glyph=glyph))
                        lines.append('</BacktrackCoverage>')
                    for j, glyphs in enumerate(r.pattern):
                        if not isinstance(glyphs, list): glyphs = [glyphs]
                        lines.append('<InputCoverage index="{index}">'.format(index=j))
                        for glyph in sorted(glyphs, key=lambda name: glyphids[name]):
                            lines.append('<Glyph value="{glyph}"/>'.format(glyph=glyph))
                        lines.append('</InputCoverage>')
                    for j, glyphs in enumerate(r.lookahead):
                        if not isinstance(glyphs, list): glyphs = [glyphs]
                        lines.append('<LookAheadCoverage index="{index}">'.format(index=j))
                        for glyph in sorted(glyphs, key=lambda name: glyphids[name]):
                            lines.append('<Glyph value="{glyph}"/>'.format(glyph=glyph))
                        lines.append('</LookAheadCoverage>')
                    lines.append('<SubstLookupRecord index="0">')
                    lines.append('<SequenceIndex value="0"/>')
                    lines.append('<LookupListIndex value="{lookup}"/><!-- {set} -->'.format(
                                    lookup=chainedlookup, set=chainedsetname))
                    lines.append('</SubstLookupRecord>')
                    lines.append('</ChainContextSubst>')

            else:
                assert False, 'not yet supported remapping set format'

            settolookup[setname] = len(lookups)
            lookups.append((setname, lines))

        # GDEF
        print >>fp, '<GDEF>'
        print >>fp, '<Version value="1.0"/>'
        print >>fp, '</GDEF>'

        # GSUB
        print >>fp, '<GSUB>'
        print >>fp, '<Version value="1.0"/>'
        print >>fp, '<ScriptList>'
        print >>fp, '<ScriptRecord index="0">'
        print >>fp, '<ScriptTag value="DFLT"/>'
        print >>fp, '<Script>'
        print >>fp, '<DefaultLangSys>'
        print >>fp, '<ReqFeatureIndex value="65535"/>'
        features = []
        for featurename, sets in self.features.items():
            print >>fp, '<FeatureIndex index="{index}" value="{feature}"/>'.format(
                    index=len(features), feature=len(features))
            features.append(featurename)
        print >>fp, '</DefaultLangSys>'
        print >>fp, '</Script>'
        print >>fp, '</ScriptRecord>'
        print >>fp, '</ScriptList>'
        print >>fp, '<FeatureList>'
        for i, featurename in enumerate(features):
            setnames = self.features[featurename]
            print >>fp, '<FeatureRecord index="{index}">'.format(index=i)
            print >>fp, '<FeatureTag value="{tag}"/>'.format(tag=featurename)
            print >>fp, '<Feature>'
            for j, setname in enumerate(setnames):
                print >>fp, '<LookupListIndex index="{index}" value="{lookup}"/><!-- {set} -->'.format(
                        index=j, lookup=settolookup[setname], set=setname)
            print >>fp, '</Feature>'
            print >>fp, '</FeatureRecord>'
        print >>fp, '</FeatureList>'
        print >>fp, '<LookupList>'
        for i, (setname, lines) in enumerate(lookups):
            print >>fp, '<Lookup index="{index}"><!-- {set} -->'.format(index=i, set=setname)
            for line in lines: print >>fp, line
            print >>fp, '</Lookup>'
        print >>fp, '</LookupList>'
        print >>fp, '</GSUB>'

        print >>fp, '</ttFont>'

    def write_json(self, fp):
        import json
        def default(o):
            if isinstance(o, set): return list(o)
            raise TypeError
        json.dump(self.__dict__, fp, separators=(',',':'), default=default)

if __name__ == '__main__':
    font = Font()
    t1 = time.time()
    try:
        current_path = None
        for pat in sys.argv[1:]:
            for path in glob.glob(pat):
                current_path = path
                with open(path) as f:
                    font.read(f)
        current_path = None
        font.resolve_glyphs()
        font.inline_glyphs()
    except ParseError as e:
        if current_path: print >>sys.stderr, current_path + ':',
        print >>sys.stderr, unicode(e).encode('utf-8')
        raise SystemExit(1)
    t2 = time.time()
    with open('unison.json', 'wb') as f:
        font.write_json(f)
    with open('unison.ttx', 'w') as f:
        font.write_ttx(f)
    t3 = time.time()
    print >>sys.stderr, '%.3fs parsing, %.3fs rendering' % (t2 - t1, t3 - t2)

