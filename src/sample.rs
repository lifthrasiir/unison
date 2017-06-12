use std::char;
use std::io;
use std::fmt;
use std::collections::BTreeSet;
use gif;
use base64;
use unicode_normalization::UnicodeNormalization;
use md5;
use external_data::{UNICODE_DATA, UDHR_SAMPLES, CONFUSABLES};
use font::*;
use contour;

struct Escape<'a>(&'a str);

impl<'a> fmt::Display for Escape<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for c in self.0.chars() {
            match c {
                '&' => write!(f, "&amp;")?,
                '"' => write!(f, "&quot;")?,
                '\'' => write!(f, "&#39;")?,
                '<' => write!(f, "&lt;")?,
                '>' => write!(f, "&gt;")?,
                _ => write!(f, "{}", c)?,
            }
        }
        Ok(())
    }
}

struct CharName(u32);

impl fmt::Display for CharName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let name = UNICODE_DATA.name(self.0);
        write!(f, "U+{:04X}", self.0)?;
        if !name.is_empty() {
            write!(f, " {}", name)?;
        }
        if let Some(c) = char::from_u32(self.0) {
            write!(f, " ({})", c)?;
        }
        Ok(())
    }
}

fn escape(s: &str) -> Escape { Escape(s) }
fn char_name(cp: u32) -> CharName { CharName(cp) }

fn hash(s: &str) -> u32 {
    let h = md5::compute(s);
    (h[0] as u32) << 24 | (h[1] as u32) << 16 | (h[2] as u32) << 8 | h[3] as u32
}

pub fn write_pgm(f: &mut io::Write, font: &Font) -> io::Result<()> {
    use std::collections::{HashMap, HashSet};

    const MAX_HEIGHT: u32 = 16;
    const LINE_WIDTH: u32 = 512;
    const NUM_GLYPHS_PER_LINE: [u32; 7] = [64, 32, 16, 8, 4, 2, 1];
    const MAX_GLYPHS_PER_LINE: u32 = 64;

    // returns the largest multiple of b (which should be 2^k for some k) <= a
    fn multiples(a: u32, b: u32) -> u32 {
        a & (-(b as i32) as u32)
    }

    // determine the width of glyphs (and check the height)
    let mut glyphs = HashMap::new(); // (width, height, list of resolved subglyphs)
    let mut unavailable_widths = HashSet::new(); // (# glyphs per line, start character)
    for (&ch, name) in font.cmap.iter() {
        let gg = font.get_glyph(name).unwrap();
        let subglyphs = font.get_subglyphs(name).unwrap();
        assert!(gg.height <= MAX_HEIGHT, "glyph `{}` is too tall", name);
        assert!(gg.width <= LINE_WIDTH, "glyph `{}` is too wide", name);
        glyphs.insert(name.to_owned(), (gg.width, gg.height, subglyphs));
        for &w in &NUM_GLYPHS_PER_LINE {
            if gg.width > LINE_WIDTH / w {
                unavailable_widths.insert((w, multiples(ch, w)));
            }
        }
    }

    // determine the position of each glyph
    let mut last = None;
    let mut row = -1i32;
    let mut gap = 0;
    let mut positions = HashMap::new(); // (row, column)
    let mut row_starts = Vec::new();
    let mut row_offset = Vec::new(); // i.e. accumulated `gap`
    for &ch in font.cmap.keys() {
        let mut current = None;
        for &w in &NUM_GLYPHS_PER_LINE {
            if !unavailable_widths.contains(&(w, multiples(ch, w))) {
                current = Some((w, multiples(ch, w)));
                break;
            }
        }
        let current = current.expect("no remaining slot for the glyph");
        let w = current.0;
        if last != Some(current) {
            if current.1 - last.map_or(0, |(_, l)| l) > MAX_GLYPHS_PER_LINE {
                gap += 8;
            }
            row += 1;
            row_starts.push(multiples(ch, w));
            row_offset.push(gap);
            last = Some(current);
        }
        positions.insert(ch, (row as u32, (ch & (w-1)) * (LINE_WIDTH / w)));
    }
    let nrows = (row + 1) as u32;

    //          +---------------------+ ^
    // __U+xxxx | a b c d e f g h ... | | 17px each + last one
    // <------> +---------------------+ v
    //  8*8px+1 ^ <-----------------> ^
    //         1px      8*64px       1px
    let imwidth = 8*8 + 1 + 1 + LINE_WIDTH + 1;
    let imheight = (MAX_HEIGHT + 1) * nrows + 1 + gap;
    let imline = {
        let mut v = Vec::new();
        v.resize(8*8 + 1, 0xff);
        v.resize(imwidth as usize, 0x80);
        v
    };

    fn render_glyphs(current: &mut [Vec<u8>], left: i32,
                     &(width, height, ref glyphs): &(u32, u32, Vec<LocatedSubglyph>), color: u8) {
        for r in 0..(height as usize) {
            for c in (left as usize)..(left as usize + width as usize) {
                current[r][c] = 0xff;
            }
        }
        for g in glyphs {
            let icolor = if g.negated & 1 != 0 { 0xff } else { color };
            for r in 0..(g.height as i32) {
                let row = (g.top as i32 + r) as usize;
                for c in 0..(g.width as i32) {
                    if g.data[(r * g.stride as i32 + c) as usize] & PX_FULL != 0 {
                        current[row][(g.left as i32 + left + c) as usize] = icolor;
                    }
                }
            }
        }
    }

    write!(f, "P5 {} {} 255\n", imwidth, imheight)?;
    f.write(&imline)?;
    let mut row = -1i32;
    let mut current: Vec<Vec<u8>> = Vec::new();
    let mut lastlabelidx = None;
    for (&ch, name) in font.cmap.iter() {
        let &(r, left) = positions.get(&ch).unwrap();
        if r as i32 != row {
            assert!(r as i32 - row == 1);
            row = r as i32;
            let row = row as usize;
            for line in &current {
                f.write(line)?;
            }
            let prevrowoffset = if row > 0 { row_offset[row-1] } else { 0 };
            for _ in prevrowoffset..row_offset[row] {
                f.write(&imline)?;
            }
            current = (0..(MAX_HEIGHT + 1)).map(|_| imline.clone()).collect();
            let mut color = 0x80;
            let labelidx = multiples(row_starts[row], MAX_GLYPHS_PER_LINE);
            if lastlabelidx != Some(labelidx) {
                lastlabelidx = Some(labelidx);
                color = 0;
            }
            let label = format!("{:>8}", &format!("U+{:04X}", row_starts[row]));
            for (i, cch) in label.chars().enumerate() {
                if cch == ' ' { continue; }
                let glyph = font.cmap.get(&(cch as u32)).and_then(|name| glyphs.get(name)).expect(
                    "no glyph defined for label"
                );
                render_glyphs(&mut current, (i * 8) as i32, glyph, color);
            }
        }
        render_glyphs(&mut current, 8*8 + 1 + 1 + left as i32, glyphs.get(name).unwrap(), 0);
    }
    for line in &current {
        f.write(line)?;
    }

    Ok(())
}

pub fn write_html(f: &mut io::Write, font: &Font) -> io::Result<()> {
    const SCALE_SHIFT: usize = 1;

    fn pixels_to_path(top: i32, left: i32, height: u32, width: u32, stride: usize, data: &[u8],
                      subpixel: bool) -> String {
        use std::fmt::Write;

        let borders = if subpixel {
            contour::track_subpixel_contour(height, width, stride, data, SCALE_SHIFT)
        } else {
            contour::track_fullpixel_contour(height, width, stride, data, SCALE_SHIFT)
        };

        let mut pathstr = String::new();
        for p in borders {
            let (mut x0, mut y0) = p[0];
            let _ = write!(&mut pathstr, "M{} {}",
                           x0 + (left << SCALE_SHIFT), y0 + (top << SCALE_SHIFT));
            for &(x, y) in &p[1..] {
                if y == y0 {
                    let _ = write!(&mut pathstr, "h{}", x - x0);
                } else if x == x0 {
                    let _ = write!(&mut pathstr, "v{}", y - y0);
                } else {
                    let _ = write!(&mut pathstr, "l{} {}", x - x0, y - y0);
                }
                x0 = x;
                y0 = y;
            }
            pathstr.push('z');
        }
        pathstr
    }

    fn print_fullpixel_image(f: &mut io::Write, name: &str, font: &Font) -> io::Result<()> {
        let glyphs = font.get_subglyphs(name)?;
        let gg = font.glyphs.get(name).unwrap();

        let map = [0, 0, 0, 0xff, 0xff, 0xff];
        let mut pixels = vec![0; (gg.width * gg.height) as usize];
        for g in glyphs {
            let icolor = if g.negated & 1 != 0 { 0 } else { 1 };
            for r in 0..(g.height as i32) {
                for c in 0..(g.width as i32) {
                    if g.data[(r * g.stride as i32 + c) as usize] & PX_FULL != 0 {
                        pixels[(g.left + c + (g.top + r) * gg.width as i32) as usize] = icolor;
                    }
                }
            }
        }

        let mut buf = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut buf, gg.width as u16, gg.height as u16, &map)?;
            enc.write_frame(&gif::Frame {
                width: gg.width as u16,
                height: gg.height as u16,
                transparent: Some(0),
                buffer: pixels.into(),
                ..Default::default()
            })?;
        }

        write!(f, "<img src='data:image/gif;base64,{}' width={} height={}>",
               base64::encode(&buf), gg.width, gg.height)
    }

    fn print_svg(f: &mut io::Write, name: &str, font: &Font, scale: u32,
                 subpixel: bool) -> io::Result<()> {
        let glyphs = font.get_subglyphs(name)?;
        let gg = font.glyphs.get(name).unwrap();

        write!(f, "<svg viewBox=\"0 0 {w} {h}\" width=\"{ww}\" height=\"{hh}\">",
            w = gg.width << SCALE_SHIFT, h = gg.height << SCALE_SHIFT,
            ww = gg.width * scale, hh = gg.height * scale,
        )?;
        for g in glyphs {
            let path = pixels_to_path(g.top, g.left, g.height, g.width,
                                      g.stride, g.data, subpixel);
            if g.negated & 1 != 0 {
                write!(f, "<g><path d='{path}' fill='#000' /></g>", path = path)?;
            } else {
                let color = (hash(&path) & 0x7f7f7f) + 0x808080;
                write!(f, "<path d='{path}' fill='#{color:06x}' />", path = path, color = color)?;
            }
        }
        write!(f, "</svg>")?;
        Ok(())
    }

    let mut all_chars: Vec<_> = font.cmap.keys().collect();
    all_chars.sort_by_key(|&&k| char::from_u32(k).into_iter().nfd().collect::<String>());

    writeln!(f, "\
        <!doctype html>\n\
        <html><head><meta charset=utf-8><title>Unison: graphic sample</title><style>\n\
        {style}</style></head><body>\n\
        <input id=sample placeholder='Input sample text here' size=40> <input type=reset id=reset value=Reset> | {nchars} characters, {nglyphs} intermediate glyphs so far | <a href='sample.png'>PNG</a> | <a href='live.html'>live</a>\n\
        <hr><div id=sampleglyphs></div><div id=glyphs>",
        nchars = font.cmap.len(),
        nglyphs = font.glyphs.len(),
        style = "\
body{background:black;color:white;line-height:1}div{color:gray}#sampleglyphs{display:none}body.sample #sampleglyphs{display:block}body.sample #glyphs{display:none}.scaled{font-size:500%}\n\
img{background:#222;vertical-align:top;opacity:0.5}img:hover,body.sample img{background:#111;opacity:1}svg{background:#111;fill:white;vertical-align:top}:target svg{background:#333}svg:hover>path,body.sample svg>path{fill:white}a svg>path{fill:gray}
",
    )?;

    let mut excluded = false;
    for &&ch in &all_chars {
        if font.exclude_from_sample.contains(&ch) {
            if !excluded {
                write!(f, "…")?;
                excluded = true;
            }
        } else {
            excluded = false;
            write!(f, "<a href='#u{:x}'><span id='sm-u{:x}' title='{}'>", ch, ch,
                   escape(&char_name(ch).to_string()))?;
            if true {
                print_svg(f, font.cmap.get(&ch).unwrap(), font, 1, false)?;
            } else {
                print_fullpixel_image(f, font.cmap.get(&ch).unwrap(), font)?;
            }
            write!(f, "</span></a>")?;
        }
    }

    writeln!(f, "<hr><span class='scaled'>")?;
    let mut excluded = false;
    for &&ch in &all_chars {
        if font.exclude_from_sample.contains(&ch) {
            if !excluded {
                write!(f, "…")?;
                excluded = true;
            }
        } else {
            excluded = false;
            write!(f, "<span id='u{:x}' title='{}'>", ch, escape(&char_name(ch).to_string()))?;
            print_svg(f, font.cmap.get(&ch).unwrap(), font, 5, true)?;
            write!(f, "</span>")?;
        }
    }

    writeln!(f,
        "</span></div><script>\n{script}</script></body></html>",
        script = "\
prevt=0;
function $(x){return document.getElementById(x)}
function f(t,h){if(t.normalize)t=t.normalize();if(prevt===t)return;prevt=t;if(!h)location.hash=t?'#!'+encodeURIComponent(t):'';$('sample').value=t;document.body.className=t?'sample':'';var sm='',bg='';for(var i=0;i<t.length;++i){var c=t.charCodeAt(i).toString(16);sm+=($('sm-u'+c)||{}).innerHTML||t[i];bg+=($('u'+c)||{}).innerHTML||t[i]}$('sampleglyphs').innerHTML=sm+'<hr><span class=scaled>'+bg+'</span>'}
(window.onhashchange=function(){var h=location.hash||'';f(h.match(/^#!/)?decodeURIComponent(h.substring(2)):'',1);return false})();
$('sample').onchange=$('sample').onkeyup=function(e){f(this.value)}
$('reset').onclick=function(){$('sample').value='';f('')}
"
    )?;
    
    Ok(())
}

pub fn write_live_html(f: &mut io::Write, font: &Font) -> io::Result<()> {
    let features =
        font.features.keys().map(|feat| format!("'{}'", feat)).collect::<Vec<_>>().join(",");
    writeln!(f, "\
        <!doctype html>\n\
        <html><head><meta charset=utf-8><title>Unison: live sample</title>\n\
        <style>\n\
        @font-face{{font-family:Unison;src:url(unison.ttf);font-feature-settings:{features}}}\n\
        {style}</style>\n\
        <script>\n\
        {script}</script>\n\
        </head><body><pre>\n\
        Hello? This is the <u>Unison</u> font.\n\
        You can play with it right here or download it <a href='unison.ttf'>here</a>.\n\
        Please note that this is in development and subject to change.\n\
        \n\
        Load: <a href='#udhr'>UDHR</a>, <a href='#confus'>Confusables</a>, <a href='#hangul'>All Hangul</a>, <a href='#all'>All Glyphs</a>\n\
        ────────────────────────────────────────────────────────────\n\
        </pre><pre id=edit>{logo}</pre><pre id=udhr class=hide>\n\
        ┌──────────────────────────────────────────────────┐\n\
        │Article 1 of Universal Declaration of Human Rights│\n\
        └──────────────────────────────────────────────────┘\n",
        features = if features.is_empty() { "inherit" } else { &features[..] },
        style = "\
pre{font-family:Unison,monospace;font-size:200%;line-height:1;margin:0;white-space:pre-wrap}pre span{background:#eee}.hide{display:none}
",
        script = "\
window.onload=function(){var e=document.getElementById('edit');e.contentEditable='true';for(var x=document.querySelectorAll('a[href^=\"#\"]'),i=0;x[i];++i)x[i].onclick=function(){e.innerHTML=document.getElementById(this.getAttribute('href').substring(1)).innerHTML;return false}}
",
        logo = r#"
888     888          d8b
888     888          Y8P
888     888
888     888 88888b.  888 .d8888b   .d88b.  88888b.
888     888 888 "88b 888 88K      d88""88b 888 "88b
888     888 888  888 888 "Y8888b. 888  888 888  888
Y88b. .d88P 888  888 888      X88 Y88..88P 888  888
 "Y88888P"  888  888 888  88888P'  "Y88P"  888  888
"#,
    )?;

    let avail: BTreeSet<_> = font.cmap.keys().map(|&c| char::from_u32(c).unwrap()).collect();

    for sample in UDHR_SAMPLES.iter() {
        if sample.required_chars.is_subset(&avail) {
            writeln!(f, "• {}: <span>{}</span>", sample.code, escape(sample.text))?;
        } else {
            assert!(!sample.text.contains("--"));
            let missing: String = sample.required_chars.difference(&avail).collect();
            write!(f, "<!--\n{}: [{}] {} -->", sample.code, escape(&missing), escape(sample.text))?;
        }
    }

    writeln!(f, "\
        </pre><pre id=confus class=hide>\n\
        ┌───────────┐\n\
        │Confusables│\n\
        └───────────┘\n\
        ")?;

    for (k, vv) in CONFUSABLES.iter() {
        let mut set: Vec<_> = vv.chars().map(|c| c.to_string()).collect();
        set.push((*k).to_owned());
        set.retain(|i| i.chars().all(|c| avail.contains(&c)));
        set.sort();
        if set.len() > 1 {
            writeln!(f, "{}",
                set.iter().map(|i| {
                    let char_names: Vec<_> =
                        i.chars().map(|c| char_name(c as u32).to_string()).collect();
                    format!("<span title='{}'>{}</span>",
                            escape(&char_names.join("\n")), escape(i))
                }).collect::<Vec<_>>().join(" "))?;
        }
    }

    writeln!(f, "\
        </pre><pre id=hangul class=hide>\n\
        ┌────────────────────┐\n\
        │All Hangul Syllables│\n\
        │ (Modern + Ancient) │\n\
        └────────────────────┘\n\
        \n\
        <div style='white-space:pre'><span><a href='#' onclick=\"{onclick}\">Render!</a></span></div>\n\
        </pre><pre id=all class=hide>\n\
        ┌────────────────────┐\n\
        │All Supported Glyphs│\n\
        └────────────────────┘\n\
        ",
        onclick = escape(r"
var p=[],i,j,k,a=[],b=[],c=[''];
function v(z,s){for(i=0;s[i];++i)for(j=s[i][0];j<=(s[i][1]||s[i][0]);++j)z.push(String.fromCharCode(j))}
v(a,[[0x115f],[0x1100,0x115e],[0xa960,0xa97c]]);
v(b,[[0x1160,0x117e],[0x119e],[0x11a1],[0x11a3,0x11a4]]);
v(c,[[0x11a8,0x11c2]]);
for(i=0;i<a.length;++i,p.push('\n'))for(j=0;j<b.length;++j,p.push('\n'))for(k=0;k<c.length;++k)p.push(a[i]+b[j]+c[k]);
this.parentNode.replaceChild(document.createTextNode(p.join('')),this);
return!1"),
    )?;

    let mut chars = String::new();
    for &ch in font.cmap.keys() {
        if chars.chars().next().map_or(false, |fch| fch as u32 >> 5 != ch >> 5) {
            writeln!(f, "<span>{}</span>", escape(&chars))?;
            chars.clear();
        }
        chars.push(char::from_u32(ch).unwrap());
    }
    if !chars.is_empty() {
        writeln!(f, "<span>{}</span>", escape(&chars))?;
    }

    writeln!(f, "</pre></body></html>")?;
    Ok(())
}

