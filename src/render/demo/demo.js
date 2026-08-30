/* demo.html's renderer.
 *
 * Everything on the page is built here from the JSON blob and the two embedded
 * faces: the page ships the *font*, not pictures of it, which is what keeps it
 * a few hundred kilobytes where the old sample.html was 6.6 MB of inline SVG.
 *
 * Two things are worth knowing before changing this file.
 *
 * 1. The grid is a code chart, not a list. A row is `cp & ~0xF` and a column is
 *    the low nibble, so a character always sits where a reader expects it and a
 *    hole in the repertoire is a visibly empty slot. Rows exist only where the
 *    block has at least one character; a block's unassigned tail is not drawn.
 * 2. Cells are rendered lazily, in chunks of CHUNK rows, through one
 *    IntersectionObserver. A filled block such as CJK Unified Ideographs is
 *    twenty thousand cells; building them all up front costs seconds and holds
 *    them all in the layout tree afterwards. Every chunk therefore knows its
 *    own height before it has any content, which is why line heights are
 *    computed here rather than measured — see metrics().
 */
(function () {
  "use strict";

  var data = JSON.parse(document.getElementById("demo-data").textContent);
  var meta = data.meta;

  var DECLARED = 1;
  var EXCLUDED = 2;
  var ZERO_ADVANCE = 4;
  /* Rows per lazily-rendered chunk. */
  var CHUNK = 16;
  /* The height of a collapsed run of excluded rows, in px; also stated in
     demo.css, and border-box makes the two the same number. */
  var GAP_H = 30;
  /* The label line under a glyph, plus the cell's own padding: the difference
     between the em box and the cell, kept in step with `--cell-h`. */
  var CELL_PAD = 22;

  /* What the page opens at, in whole multiples of the pixel grid. */
  var INITIAL_ZOOM = 2;

  var state = {
    mode: "bitmap",
    /* The bitmap face is only crisp at whole multiples of the pixel grid, so
       it is zoomed in steps rather than sized freely. */
    zoom: INITIAL_ZOOM,
    /* Freely sized, but opening at the same em as the bitmap face does:
       switching between the two is for comparing them, and a switch that also
       changed the size would make that a worse comparison. Derived rather than
       stated, so it stays true for a font whose pixel grid is not this one's. */
    size: meta.height * INITIAL_ZOOM
  };

  function em() {
    return state.mode === "bitmap" ? meta.height * state.zoom : state.size;
  }

  function metrics() {
    var e = em();
    return { em: e, rowH: e + CELL_PAD, gapH: GAP_H };
  }

  /* ---- the line model ---------------------------------------------------- */

  /* One block's runs turned into what the grid actually draws: rows keyed by
     `cp & ~0xF`, with a run of wholly excluded rows collapsed into one marker.
     The exclusion rule is the editor's — `exclude-from-sample` hides a row only
     when every character on it is excluded, so an excluded range still reads
     differently from a range the source never mentions. */
  function linesOf(block) {
    var rows = [];
    var cur = null;
    block.runs.forEach(function (run) {
      for (var cp = run[0]; cp < run[0] + run[1]; cp++) {
        var base = cp - (cp % 16);
        if (!cur || cur.base !== base) {
          cur = { base: base, cells: new Array(16), excluded: true };
          for (var i = 0; i < 16; i++) cur.cells[i] = -1;
          rows.push(cur);
        }
        cur.cells[cp % 16] = run[2];
        if (!(run[2] & EXCLUDED)) cur.excluded = false;
      }
    });
    var out = [];
    rows.forEach(function (r) {
      if (!r.excluded) {
        out.push(r);
        return;
      }
      var last = out[out.length - 1];
      if (last && last.gap) last.gap += 1;
      else out.push({ gap: 1 });
    });
    return out;
  }

  /* ---- cell markup ------------------------------------------------------- */

  function hex(cp, pad) {
    var s = cp.toString(16).toUpperCase();
    while (s.length < pad) s = "0" + s;
    return s;
  }

  /* The jamo short names, which are what a Hangul syllable's character name is
     made of: `HANGUL SYLLABLE` + lead + vowel + trail. Eleven thousand names
     for three lines of table. */
  var JAMO_L = "G,GG,N,D,DD,R,M,B,BB,S,SS,,J,JJ,C,K,T,P,H".split(",");
  var JAMO_V = "A,AE,YA,YAE,EO,E,YEO,YE,O,WA,WAE,OE,YO,U,WEO,WE,WI,YU,EU,YI,I".split(",");
  var JAMO_T = ",G,GG,GS,N,NJ,NH,D,L,LG,LM,LB,LS,LT,LP,LH,M,B,BS,S,SS,NG,J,C,K,T,P,H".split(",");

  /* What to call a code point. The blob carries only the names that cannot be
     spelled here: the ideographs are a prefix and their own code point, and the
     Hangul syllables are their jamo, so both are composed rather than sent —
     and, being composed, they are available for undeclared cells too, which the
     blob says nothing about. */
  function nameOf(cp) {
    if (cp >= 0xac00 && cp <= 0xd7a3) {
      var i = cp - 0xac00;
      return "HANGUL SYLLABLE " + JAMO_L[Math.floor(i / 588)] +
        JAMO_V[Math.floor(i / 28) % 21] + JAMO_T[i % 28];
    }
    var named = data.names[hex(cp, 1)];
    if (named) return named;
    for (var r = 0; r < data.name_runs.length; r++) {
      var run = data.name_runs[r];
      if (cp < run[0]) break;
      if (cp < run[0] + run[1]) return run[2] + hex(cp, 4);
    }
    return "";
  }

  function esc(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  }

  /* A control character has no glyph a browser will take from the font — it
     substitutes or drops one — and a literal newline in a `white-space: pre`
     box would break the row outright. The cell stays a declared cell; only its
     glyph box is left empty. */
  function isControl(cp) {
    return cp < 0x20 || (cp >= 0x7f && cp <= 0x9f) || cp === 0x2028 || cp === 0x2029;
  }

  function cellHtml(cp, flags) {
    if (flags < 0) return '<div class="cell empty"></div>';
    var declared = (flags & DECLARED) !== 0;
    var name = nameOf(cp);
    var title = "U+" + hex(cp, 4) + (name ? " " + name : "") + (declared ? "" : " \u2014 not in the font");
    var glyph = declared && !isControl(cp) ? esc(String.fromCodePoint(cp)) : "";
    /* A character the font gives no advance draws on nothing and its cell reads
       as empty. The circle in front of it is the same one the editor puts there
       (`crate::editor::annotations`), and drawn the same way — as a shape, not
       by writing U+25CC, so that a fault in the font being shown cannot take
       the placeholder with it. */
    if (glyph && (flags & ZERO_ADVANCE) !== 0) glyph = '<span class="dc"></span>' + glyph;
    return (
      '<div class="cell' + (declared ? "" : " missing") + '" title="' + esc(title) + '">' +
      '<span class="g">' + glyph + "</span>" +
      '<span class="n">' + hex(cp % 256, 2) + "</span>" +
      "</div>"
    );
  }

  function lineHtml(line) {
    if (line.gap) {
      return '<div class="gap" style="height:' + GAP_H + 'px">' + line.gap +
        (line.gap === 1 ? " row" : " rows") + " excluded from the sample</div>";
    }
    var label = hex(line.base, 4);
    var alt = (line.base >> 4) % 2 ? " alt" : "";
    var html = '<div class="row' + alt + '"><div class="gut">' + label.slice(0, -1) + "_</div>";
    for (var i = 0; i < 16; i++) html += cellHtml(line.base + i, line.cells[i]);
    return html + "</div>";
  }

  /* ---- page -------------------------------------------------------------- */

  var chunks = [];
  var observer = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      if (!entry.isIntersecting) return;
      var chunk = entry.target;
      observer.unobserve(chunk);
      chunk.innerHTML = chunk.__lines.map(lineHtml).join("");
    });
  }, { rootMargin: "800px 0px" });

  function chunkHeight(lines, m) {
    var h = 0;
    lines.forEach(function (l) { h += l.gap ? m.gapH : m.rowH; });
    return h;
  }

  function applyMetrics() {
    var m = metrics();
    var root = document.documentElement.style;
    root.setProperty("--em", m.em + "px");
    document.body.classList.toggle("bitmap", state.mode === "bitmap");
    chunks.forEach(function (chunk) {
      chunk.style.height = chunkHeight(chunk.__lines, m) + "px";
    });
  }

  function ruler() {
    var html = '<div class="ruler"><div></div>';
    for (var i = 0; i < 16; i++) html += "<div>" + i.toString(16).toUpperCase() + "</div>";
    return html + "</div>";
  }

  function buildBlock(block, index) {
    var section = document.createElement("section");
    section.className = "block";
    section.id = "b" + index;

    var head = '<div class="block-head"><h2>' + esc(block.name) + "</h2>";
    if (block.range) head += '<span class="range">' + esc(block.range) + "</span>";
    if (block.coverage) {
      var pct = block.coverage[1] ? Math.round((block.coverage[0] / block.coverage[1]) * 100) : 0;
      head += '<div class="cov"><span>' + block.coverage[0] + " / " + block.coverage[1] +
        " · " + pct + '%</span><div class="bar"><i style="width:' + pct + '%"></i></div></div>';
    }
    head += "</div>";
    section.innerHTML = head + ruler() + '<div class="rows"></div>';

    var rows = section.querySelector(".rows");
    var lines = linesOf(block);
    var m = metrics();
    for (var i = 0; i < lines.length; i += CHUNK) {
      var slice = lines.slice(i, i + CHUNK);
      var chunk = document.createElement("div");
      chunk.className = "chunk";
      chunk.__lines = slice;
      chunk.style.height = chunkHeight(slice, m) + "px";
      rows.appendChild(chunk);
      chunks.push(chunk);
      observer.observe(chunk);
    }
    return section;
  }

  function header() {
    var el = document.createElement("header");
    var declared = 0, total = 0;
    data.blocks.forEach(function (b) {
      if (!b.coverage) return;
      declared += b.coverage[0];
      total += b.coverage[1];
    });
    var options = data.blocks.map(function (b, i) {
      return '<option value="b' + i + '">' + esc(b.name) + "</option>";
    }).join("");
    el.innerHTML =
      '<div class="brand"><h1>' + esc(meta.family + " " + meta.subfamily) + "</h1>" +
      '<span class="ver">' + esc(meta.version) + "</span></div>" +
      '<div class="stats">' +
      '<span class="chip"><b>' + meta.mapped + "</b> characters</span>" +
      '<span class="chip"><b>' + data.blocks.length + "</b> blocks</span>" +
      '<span class="chip"><b>' + meta.height + "</b> px em</span>" +
      (total ? '<span class="chip">covers <b>' + Math.round((declared / total) * 100) + "%</b> of them</span>" : "") +
      "</div>" +
      '<div class="controls">' +
      '<div class="segmented" role="group" aria-label="rendering">' +
      '<button id="m-bitmap" aria-pressed="true">Bitmap</button>' +
      '<button id="m-vector" aria-pressed="false">Vector</button>' +
      "</div>" +
      '<label class="size">Size<input id="size" type="range" min="1" max="6" step="1" value="2">' +
      '<span class="val" id="size-val"></span></label>' +
      '<select class="jump" id="jump"><option value="">Jump to block…</option>' + options + "</select>" +
      "</div>";
    return el;
  }

  function sizeControl() {
    var input = document.getElementById("size");
    if (state.mode === "bitmap") {
      input.min = 1; input.max = 6; input.step = 1; input.value = state.zoom;
    } else {
      input.min = 16; input.max = 96; input.step = 4; input.value = state.size;
    }
    document.getElementById("size-val").textContent =
      state.mode === "bitmap" ? state.zoom + "× (" + em() + "px)" : em() + "px";
  }

  function setMode(mode) {
    state.mode = mode;
    document.getElementById("m-bitmap").setAttribute("aria-pressed", String(mode === "bitmap"));
    document.getElementById("m-vector").setAttribute("aria-pressed", String(mode === "vector"));
    sizeControl();
    applyMetrics();
  }

  function footer() {
    var el = document.createElement("footer");
    el.innerHTML =
      '<div class="legend">' +
      "<span><i class=\"declared\"></i>drawn by the font</span>" +
      "<span><i class=\"missing\"></i>a character the font does not have</span>" +
      "<span>an empty slot is a code point Unicode assigns to nothing</span>" +
      "</div>";
    return el;
  }

  document.body.appendChild(header());
  var main = document.createElement("main");
  data.blocks.forEach(function (b, i) { main.appendChild(buildBlock(b, i)); });
  document.body.appendChild(main);
  document.body.appendChild(footer());

  document.getElementById("m-bitmap").onclick = function () { setMode("bitmap"); };
  document.getElementById("m-vector").onclick = function () { setMode("vector"); };
  document.getElementById("size").oninput = function () {
    if (state.mode === "bitmap") state.zoom = +this.value;
    else state.size = +this.value;
    sizeControl();
    applyMetrics();
  };
  document.getElementById("jump").onchange = function () {
    var el = this.value && document.getElementById(this.value);
    if (el) el.scrollIntoView({ block: "start" });
    this.value = "";
  };

  sizeControl();
  applyMetrics();
})();
