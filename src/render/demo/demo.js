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
 * 3. The chart and the sample panel share one size and one drawing — the two
 *    header controls drive both — because the panel is there to answer what a
 *    code chart cannot: what the font looks like as running text.
 */
(function () {
  "use strict";

  var data = JSON.parse(document.getElementById("demo-data").textContent);
  var meta = data.meta;

  var DECLARED = 1;
  var ZERO_ADVANCE = 2;
  /* Rows per lazily-rendered chunk. */
  var CHUNK = 16;
  /* The label line under a glyph, plus the cell's own padding: the difference
     between the em box and the cell, kept in step with `--cell-h`. */
  var CELL_PAD = 22;

  /* What the page opens at, in whole multiples of the pixel grid. */
  var INITIAL_ZOOM = 2;
  /* The zoom steps the bitmap face is offered at, and so — multiplied by the
     pixel grid — the range the vector face is sized over too. */
  var MIN_ZOOM = 1;
  var MAX_ZOOM = 6;

  /* The size is *one* number for both drawings, in px of em. Switching between
     bitmap and vector is for comparing them, and a switch that also changed the
     size would make that a worse comparison — so the mode changes what is
     drawn, never how big it is drawn.
     The one thing a switch may do is round: the bitmap face is only crisp at
     whole multiples of the pixel grid, so entering bitmap mode snaps the shared
     size to the nearest multiple of `meta.height` (a tie rounds up), and leaves
     it there. Everything is derived from `meta.height` rather than stated, so it
     stays true for a font whose pixel grid is not this one's. */
  var state = {
    mode: "bitmap",
    em: meta.height * INITIAL_ZOOM
  };

  function em() {
    return state.em;
  }

  /* The shared size read as a bitmap zoom: nearest whole multiple of the pixel
     grid, ties up (`Math.round` rounds a positive half up), clamped to the steps
     the control offers. */
  function snapZoom(px) {
    return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Math.round(px / meta.height)));
  }

  /* A block of more than this many code points is folded in the middle, and
     this many rows are left showing at each end. A code chart is read by
     scrolling, so a block of twenty thousand ideographs otherwise puts a
     thousand identical rows between its neighbours; the fold keeps its two
     ends — where a block's own character is — and puts everything between
     them one click away. The source has no say in it: `exclude-from-sample`
     is what says this in the editor and in sample.html, and this page ignores
     it, since a rule stated once here covers every long block rather than the
     ones a font's author happened to name. */
  var FOLD_OVER = 0x100;
  /* Half the fold's threshold, in rows: a block right at the threshold shows
     every row it has, and one past it hides at least one. */
  var FOLD_EDGE = FOLD_OVER / 32;

  function metrics() {
    var e = em();
    return { em: e, rowH: e + CELL_PAD };
  }

  /* ---- the line model ---------------------------------------------------- */

  /* One block's runs, as the blob writes them: `gap,len,flags` in hex,
     separated by `;`, a gap being the distance past the end of the run before
     it (past the block's start, for the first). Written out in full the runs
     were a third of this page. */
  function eachRun(block, fn) {
    if (!block.runs) return;
    var cp = block.start;
    block.runs.split(";").forEach(function (tok) {
      var f = tok.split(",");
      var start = cp + parseInt(f[0], 16);
      var len = parseInt(f[1], 16);
      fn(start, len, parseInt(f[2], 16));
      cp = start + len;
    });
  }

  /* One block's runs turned into what the grid actually draws: rows keyed by
     `cp & ~0xF`, in code point order. A row exists as soon as one character on
     it does; the slots around it are empty cells. */
  function linesOf(block) {
    var rows = [];
    var cur = null;
    eachRun(block, function (start, len, flags) {
      for (var cp = start; cp < start + len; cp++) {
        var base = cp - (cp % 16);
        if (!cur || cur.base !== base) {
          cur = { base: base, cells: new Array(16) };
          for (var i = 0; i < 16; i++) cur.cells[i] = -1;
          rows.push(cur);
        }
        cur.cells[cp % 16] = flags;
      }
    });
    return rows;
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

  /* The names the blob does write out, undone: code points delta-coded, and
     each name stored as one base-62 digit saying what it shares with the name
     before it plus the rest. Character names are written to sort together, so
     that halves them — see `DemoNames` on the Rust side. */
  var B62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  var names = (function (blob) {
    var out = new Map();
    if (!blob.cps) return out;
    var deltas = blob.cps.split(",");
    var cp = 0, prev = "";
    for (var i = 0; i < deltas.length; i++) {
      cp += parseInt(deltas[i], 16);
      var e = blob.text[i];
      prev = prev.slice(0, B62.indexOf(e.charAt(0))) + e.slice(1);
      out.set(cp, prev);
    }
    return out;
  })(data.names);

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
    var named = names.get(cp);
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
    var label = hex(line.base, 4);
    var alt = (line.base >> 4) % 2 ? " alt" : "";
    var html = '<div class="row' + alt + '"><div class="gut">' + label.slice(0, -1) + "_</div>";
    for (var i = 0; i < 16; i++) html += cellHtml(line.base + i, line.cells[i]);
    return html + "</div>";
  }

  /* ---- the sample panel -------------------------------------------------- */

  /* The panel is pinned to the bottom of the window and the text in it is
     editable, which is the whole point of it: a specimen answers "does the font
     have this character", and running text answers "does it read", and the
     second question is one the reader has to be able to ask with their own
     words. What the reader types is *theirs*, so it is kept — per sample, in
     sessionStorage, which is the lifetime that matches a page whose state is
     otherwise all in the URL-less DOM: it survives a reload and a jump to a
     block, and it does not follow the reader into next week.
     Every access is guarded: a browser set to block site data throws on the
     accessor itself, and a specimen must still work with no storage at all. */
  var SS = "unison-demo/";
  function ssGet(key) {
    try { return sessionStorage.getItem(SS + key); } catch (e) { return null; }
  }
  function ssSet(key, val) {
    try { sessionStorage.setItem(SS + key, val); } catch (e) { /* no storage */ }
  }
  function ssDel(key) {
    try { sessionStorage.removeItem(SS + key); } catch (e) { /* no storage */ }
  }

  /* What to call a translation. The blob carries the UDHR's own key for it —
     an ISO 639-3 code, sometimes with a variant suffix — and never a language
     name: the browser already has the whole table, and shipping five hundred
     names to write a hundred and nineteen of them is the same mistake the
     character names on this page were modelled to avoid. A code the browser
     cannot name is left as a code, in capitals so that it reads as one. */
  function displayNamesOf(type) {
    try {
      return new Intl.DisplayNames(["en"], { type: type, fallback: "code" });
    } catch (e) {
      return null;
    }
  }
  var languageNames = displayNamesOf("language");
  var scriptNames = displayNamesOf("script");

  function named(table, code, pattern) {
    if (!table || !pattern.test(code)) return null;
    var name = null;
    try { name = table.of(code); } catch (e) { name = null; }
    return name && name !== code ? name : null;
  }

  function langName(id) {
    var parts = id.split("_");
    var name = named(languageNames, parts[0], /^[a-z]{2,3}$/) || parts[0].toUpperCase();
    /* A suffix is the UDHR's own qualifier on the translation, and most of them
       are the script it is written in — the two Serbian texts differ in nothing
       else. A script code is spelled out like the language; anything else
       (`asante`, `polytonic`, a year) is the data's own word and stands. */
    for (var i = 1; i < parts.length; i++) {
      var part = parts[i];
      var script = named(scriptNames, part.charAt(0).toUpperCase() + part.slice(1), /^[A-Z][a-z]{3}$/);
      name += " (" + (script || part) + ")";
    }
    return name;
  }

  var samples = data.samples || [];
  /* Every offered text by the key its edits are stored under. `custom` is the
     one entry no group carries: the reader's own text, which is what the panel
     shows when nothing is selected. */
  var sampleTexts = { custom: "" };
  var CUSTOM = "custom";
  var current = CUSTOM;

  function sampleList() {
    if (!samples.length) {
      return '<p class="s-none">No sample text was built into this page.</p>';
    }
    return samples.map(function (group, gi) {
      var items = group.items.map(function (item) {
        var key = gi + "/" + item.id;
        sampleTexts[key] = item.text;
        return '<button type="button" class="s-item" data-key="' + esc(key) +
          '" aria-pressed="false">' + esc(langName(item.id)) + "</button>";
      }).join('<span class="s-sep">; </span>');
      /* One heading per body of built-in data, with its items run together
         under it: a hundred and nineteen translations of one paragraph are one
         entry on this list, not a hundred and nineteen. They keep the order the
         blob wrote them in, which is the order they were *chosen* in — each one
         earns its place by drawing something no earlier one did — so the head
         of the list is where the widely-read languages are. */
      return '<div class="s-group"><h3 title="' + esc(group.note) + '">' +
        esc(group.title) + "</h3>" +
        '<p class="s-items">' + items + "</p></div>";
    }).join("");
  }

  function samplePanel() {
    var el = document.createElement("section");
    el.className = "samples";
    el.innerHTML =
      '<div class="s-head">' +
      '<button type="button" class="s-toggle" id="s-toggle" aria-expanded="true" aria-controls="s-body">' +
      '<span class="s-caret" aria-hidden="true"></span>Sample</button>' +
      '<span class="s-current" id="s-current"></span>' +
      '<button type="button" class="s-revert" id="s-revert">Revert</button>' +
      "</div>" +
      '<div class="s-body" id="s-body">' +
      '<div class="s-list">' + sampleList() + "</div>" +
      '<textarea class="s-text" id="s-text" dir="auto" spellcheck="false" ' +
      'aria-label="sample text"></textarea>' +
      "</div>";
    return el;
  }

  /* Show one sample. What the reader last typed into it wins over what the blob
     carries — an edit is not undone by looking at something else and coming
     back — and `Revert` is what puts the built-in text back. */
  function selectSample(key) {
    if (!(key in sampleTexts)) key = CUSTOM;
    current = key;
    ssSet("sample", key);
    var stored = ssGet("text/" + key);
    var text = document.getElementById("s-text");
    text.value = stored === null ? sampleTexts[key] : stored;
    var label = key === CUSTOM ? "your own text" : langName(key.slice(key.indexOf("/") + 1));
    document.getElementById("s-current").textContent = label;
    document.getElementById("s-revert").hidden = key === CUSTOM;
    Array.prototype.forEach.call(document.querySelectorAll(".s-item"), function (b) {
      b.setAttribute("aria-pressed", String(b.dataset.key === key));
    });
  }

  function setCollapsed(collapsed) {
    document.body.classList.toggle("s-collapsed", collapsed);
    document.getElementById("s-toggle").setAttribute("aria-expanded", String(!collapsed));
    document.getElementById("s-body").hidden = collapsed;
    ssSet("collapsed", collapsed ? "1" : "0");
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
    return lines.length * m.rowH;
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

  /* One run of lines, appended as lazily-rendered chunks. `before` is the node
     to insert them in front of — the fold marker, when a fold is opening — or
     null to append at the end. */
  function appendChunks(rows, lines, before, m) {
    for (var i = 0; i < lines.length; i += CHUNK) {
      var slice = lines.slice(i, i + CHUNK);
      var chunk = document.createElement("div");
      chunk.className = "chunk";
      chunk.__lines = slice;
      chunk.style.height = chunkHeight(slice, m) + "px";
      rows.insertBefore(chunk, before || null);
      chunks.push(chunk);
      observer.observe(chunk);
    }
  }

  /* The marker standing in for a folded block's middle. It says how much is
     behind it and where, since the two ends on either side of it are all the
     reader has to place it by, and opening it is one click: the hidden rows
     take the marker's place, still in chunks, so opening the CJK block costs
     what scrolling to it would have. A fold does not close again — a reader
     who opened one is looking for something in it. */
  function foldMarker(hidden, rows) {
    var first = hidden[0].base;
    var last = hidden[hidden.length - 1].base + 15;
    var el = document.createElement("button");
    el.type = "button";
    el.className = "fold";
    el.innerHTML =
      "<span>" + hidden.length + " rows hidden · U+" + hex(first, 4) +
      "\u2013U+" + hex(last, 4) + '</span><span class="more">Show all</span>';
    el.onclick = function () {
      appendChunks(rows, hidden, el, metrics());
      el.remove();
    };
    return el;
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
    if (lines.length > 2 * FOLD_EDGE) {
      var hidden = lines.slice(FOLD_EDGE, lines.length - FOLD_EDGE);
      appendChunks(rows, lines.slice(0, FOLD_EDGE), null, m);
      rows.appendChild(foldMarker(hidden, rows));
      appendChunks(rows, lines.slice(lines.length - FOLD_EDGE), null, m);
    } else {
      appendChunks(rows, lines, null, m);
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
    /* One control, one shared value; only its granularity is the mode's. The
       vector face steps finely over the same span the zoom steps cover, so a
       size reachable in one mode is reachable in the other. */
    if (state.mode === "bitmap") {
      input.min = MIN_ZOOM;
      input.max = MAX_ZOOM;
      input.step = 1;
      input.value = state.em / meta.height;
    } else {
      input.min = meta.height * MIN_ZOOM;
      input.max = meta.height * MAX_ZOOM;
      input.step = Math.max(1, Math.round(meta.height / 4));
      input.value = state.em;
    }
    document.getElementById("size-val").textContent =
      state.mode === "bitmap"
        ? state.em / meta.height + "× (" + state.em + "px)"
        : state.em + "px";
  }

  function setMode(mode) {
    state.mode = mode;
    if (mode === "bitmap") state.em = snapZoom(state.em) * meta.height;
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
  /* Last in the document, so that `position: sticky; bottom: 0` pins it over
     the chart while there is chart left and lands it under the footer at the
     end — no fixed panel, and so no height for the page to be told about. */
  document.body.appendChild(samplePanel());

  document.getElementById("m-bitmap").onclick = function () { setMode("bitmap"); };
  document.getElementById("m-vector").onclick = function () { setMode("vector"); };
  document.getElementById("size").oninput = function () {
    state.em = state.mode === "bitmap" ? +this.value * meta.height : +this.value;
    sizeControl();
    applyMetrics();
  };
  document.getElementById("jump").onchange = function () {
    var el = this.value && document.getElementById(this.value);
    if (el) el.scrollIntoView({ block: "start" });
    this.value = "";
  };

  Array.prototype.forEach.call(document.querySelectorAll(".s-item"), function (b) {
    b.onclick = function () { selectSample(b.dataset.key); };
  });
  document.getElementById("s-text").oninput = function () {
    ssSet("text/" + current, this.value);
  };
  document.getElementById("s-revert").onclick = function () {
    ssDel("text/" + current);
    selectSample(current);
    document.getElementById("s-text").focus();
  };
  document.getElementById("s-toggle").onclick = function () {
    setCollapsed(!document.body.classList.contains("s-collapsed"));
  };

  /* The panel opens on what the reader was last looking at, and on the first
     entry of the first group otherwise — the sample worth showing unasked is
     the one the data put first. */
  setCollapsed(ssGet("collapsed") === "1");
  selectSample(ssGet("sample") || (samples.length ? "0/" + samples[0].items[0].id : CUSTOM));
  sizeControl();
  applyMetrics();
})();
