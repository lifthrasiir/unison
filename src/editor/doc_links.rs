use crate::document::{DocLine, NamePartsMap};
use crate::document_io::{tokenize_tokens, tokenize_with_spans};
use crate::editor::line_fields::{FieldRole, LineField, classify_line};
use crate::pattern::NamePattern;

/// Whether the written name `token` is a pattern that denotes `name`.
///
/// Expanded exactly as the pipeline expands it — name parts substituted first,
/// then the grammar of the context the token sits in: a glyph block name reads
/// a top-level `a|b` as two verbatim names, an operand reads it as one group
/// (`pattern.rs` spells the difference out). Searching for `foo` therefore
/// lists a `glyph fo(o|q)` line, and the span pushed for it is the pattern
/// token as written, so that is what the pane highlights.
///
/// The same test decides where a *definition* is: a glyph block written as a
/// pattern declares every name it expands to, and that is the line a click on
/// one of those names has to reach. Most of `font/` is stated that way — the
/// whole of `han.unf` is seven `glyph han-($#…)` blocks — so matching a name
/// literally would leave those glyphs with no definition to go to at all.
///
/// `parts` is the app's collected map, which holds the *unqualified*
/// `name-parts` bindings only — a pattern spelled with a slice-qualified
/// variable expands to nothing here and is listed only if it matches
/// literally. That is the same map every other editor feature reads
/// (`app/resize.rs`, the derived data in `app/background.rs`), so the search
/// agrees with them rather than being right on its own.
///
/// `captures` is the other: a `$-N` on a `ref` line names a group of the
/// *block header* above it, which the line likewise does not say. The caller
/// carries the groups in force down the file, the way it carries the `@` base
/// — see [`crate::app::search::block_captures`].
///
/// `exists` is the one context where a token's meaning comes from a *different*
/// line: `glyph han-($1)` under `exists han-([0-9a-f]{4,5}):15x16` denotes
/// `han-4e00` and says so nowhere on its own line. `exists` is the pattern in
/// force there, carried by [`crate::exists::Carry`], and the two are combined
/// by [`crate::exists::template_denotes`].
pub fn pattern_denotes(
    token: &str,
    is_def: bool,
    name: &str,
    parts: &NamePartsMap,
    exists: Option<&str>,
    captures: &[Vec<String>],
) -> bool {
    if !token.contains(['(', '|', '$', '*']) {
        return false;
    }
    if let Some(pattern) = exists.filter(|_| crate::exists::mentions_capture(token)) {
        return crate::exists::template_denotes(pattern, token, name).unwrap_or(false);
    }
    let substituted = crate::pattern::substitute_name_parts_and_captures(token, parts, captures);
    if substituted == name {
        return true;
    }
    if !crate::document::is_name_pattern(&substituted) {
        return false;
    }
    let parsed = if is_def {
        NamePattern::parse(&substituted)
    } else {
        NamePattern::parse_element(&substituted)
    };
    parsed.is_ok_and(|p| p.matches(name))
}

#[derive(Clone, Debug)]
pub(crate) struct LinkSpan {
    pub col_start: usize,
    pub col_end: usize,
    pub target: String,
    pub kind: LinkTargetKind,
    /// The token *is* the name's declaration rather than a use of it, so there
    /// is nowhere for a Ctrl/Cmd+click to go. The host lists every appearance
    /// instead — which is also what a reference whose target does not exist
    /// falls back to, so the two arrive at the same place by different routes.
    pub is_def: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTargetKind {
    Glyph,
    NameParts,
    Remap,
    Color,
    /// An `anchor` name. Anchors are matched by name across glyphs and are
    /// declared nowhere in particular, so these never navigate.
    Anchor,
    /// An OpenType feature tag, likewise declared nowhere in particular.
    Feature,
    /// A typeface id, declared by a `face` line.
    Face,
    /// A slice id, declared by a `slice` line.
    Slice,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenameKind {
    Glyph,
    NameParts,
    Point,
    Color,
    Face,
    Slice,
    RemapGroup,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct RenameTarget {
    pub name: String,
    pub kind: RenameKind,
    pub col_start: usize,
    pub col_end: usize,
}

pub(crate) fn find_name_col_range_after_prefix(line: &str, prefix: &str) -> Option<(usize, usize)> {
    let trimmed = line.trim_start();
    let leading_chars = line.chars().count() - trimmed.chars().count();
    let spans = tokenize_with_spans(trimmed).ok()?;
    if spans.first().is_none_or(|t| t.value != prefix.trim()) {
        return None;
    }
    let name_span = spans.get(1)?;
    if name_span.value.is_empty() {
        return None;
    }
    Some((
        leading_chars + name_span.raw_start,
        leading_chars + name_span.raw_end,
    ))
}

pub(crate) fn scan_dollar_refs(text: &str, base_col: usize, out: &mut Vec<LinkSpan>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            if i > start + 1 {
                let var_name: String = chars[start..i].iter().collect();
                out.push(LinkSpan {
                    col_start: base_col + start,
                    col_end: base_col + i,
                    target: var_name,
                    kind: LinkTargetKind::NameParts,
                    is_def: false,
                });
            }
        } else {
            i += 1;
        }
    }
}

fn extract_name_parts_vars(name: &str, base_col: usize) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    scan_dollar_refs(name, base_col, &mut links);
    links
}

fn extract_glyph_and_parts_links(
    name: &str,
    base_col: usize,
    at_base: Option<&str>,
) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    let has_dollar = name.contains('$');
    if has_dollar {
        links.extend(extract_name_parts_vars(name, base_col));
    }
    let name_char_len = name.chars().count();
    if name_char_len > 0 {
        links.push(LinkSpan {
            col_start: base_col,
            col_end: base_col + name_char_len,
            // The link goes where the *glyph* is, so an `@` name links to what
            // it expands to and not to a name nothing declares.
            target: crate::document::expand_at_name(name, at_base),
            kind: LinkTargetKind::Glyph,
            is_def: false,
        });
    }
    links
}

fn whole_field_link(f: &LineField, kind: LinkTargetKind, is_def: bool) -> LinkSpan {
    LinkSpan {
        col_start: f.col_start,
        col_end: f.col_end,
        target: f.token.clone(),
        kind,
        is_def,
    }
}

/// The words a `// …` comment is made of that name a glyph the font actually
/// has.
///
/// Prose is not a directive, so nothing on a comment says which of its words is
/// a name; the *existence* of the glyph is the whole test. A word here is a
/// maximal run of glyph-name characters (`crate::pattern::is_valid_glyph_name`'s
/// set), which is finer than whitespace: `han-4e00을` in a Korean sentence and
/// `foo,` in a list both yield the name alone. A run that names nothing is left
/// as plain text rather than becoming a dead link — unlike a `ref`, where a
/// name that resolves to nothing is a fault worth clicking on.
///
/// The predicate is the editor's resolved glyph table (`EditorEnv::named_glyphs`),
/// the same set completion offers, so a comment links exactly to what could
/// have been written on a `ref` line.
pub(crate) fn extract_comment_links(
    line: &str,
    is_glyph: &dyn Fn(&str) -> bool,
    out: &mut Vec<LinkSpan>,
) {
    let Some(comment) = crate::document_io::split_comment(line).1 else {
        return;
    };
    let base_col = line.chars().count() - comment.chars().count();
    let chars: Vec<char> = comment.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !is_glyph_name_char(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_glyph_name_char(chars[i]) {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        if is_glyph(&word) {
            out.push(LinkSpan {
                col_start: base_col + start,
                col_end: base_col + i,
                target: word,
                kind: LinkTargetKind::Glyph,
                is_def: false,
            });
        }
    }
}

fn is_glyph_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':')
}

/// `at_base` is the `@` base in force on this line — see
/// [`crate::document::at_base_at_line`], which is what every caller computes it
/// with.
pub(crate) fn extract_line_links(line: &str, at_base: Option<&str>) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    for f in classify_line(line) {
        match f.role {
            FieldRole::GlyphRef => {
                links.extend(extract_glyph_and_parts_links(
                    &f.token,
                    f.col_start,
                    at_base,
                ));
            }
            // A definition is not a link to itself, but it is still worth
            // clicking: with nothing to go to, the click lists the name's uses.
            // The `$var`s inside a pattern name stay ordinary references, and a
            // pattern name as a whole is not a name anything can refer to, so
            // only plain names get the definition link.
            FieldRole::GlyphDef => {
                links.extend(extract_name_parts_vars(&f.token, f.col_start));
                if !f.token.contains('$') && !f.token.contains('(') {
                    let mut link = whole_field_link(&f, LinkTargetKind::Glyph, true);
                    link.target = crate::document::expand_at_name(&link.target, at_base);
                    links.push(link);
                }
            }
            FieldRole::NamePartsValue => {
                scan_dollar_refs(&f.token, f.col_start, &mut links);
            }
            FieldRole::ColorRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Color, false));
            }
            FieldRole::RemapGroupRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Remap, false));
            }
            FieldRole::NamePartsDef => {
                links.push(whole_field_link(&f, LinkTargetKind::NameParts, true));
            }
            FieldRole::ColorDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Color, true));
            }
            FieldRole::RemapGroupDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Remap, true));
            }
            FieldRole::FeatureDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Feature, true));
            }
            FieldRole::FaceDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Face, true));
            }
            FieldRole::FaceRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Face, false));
            }
            FieldRole::SliceDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Slice, true));
            }
            FieldRole::SliceRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Slice, false));
            }
            // The `+`/`-` prefix says which side of the attachment this is, and
            // both sides are the same anchor, so the link drops it.
            FieldRole::PointDef => {
                let mut link = whole_field_link(&f, LinkTargetKind::Anchor, true);
                link.target = link
                    .target
                    .strip_prefix(['+', '-'])
                    .unwrap_or(&link.target)
                    .to_string();
                links.push(link);
            }
        }
    }
    links
}

fn scan_dollar_ref_at(text: &str, base_col: usize, col: usize) -> Option<RenameTarget> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            if i > start + 1 {
                let abs_start = base_col + start;
                let abs_end = base_col + i;
                if col >= abs_start && col <= abs_end {
                    let var_name: String = chars[start..i].iter().collect();
                    return Some(RenameTarget {
                        name: var_name,
                        kind: RenameKind::NameParts,
                        col_start: abs_start,
                        col_end: abs_end,
                    });
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// A glyph-name field is renameable as a whole when it is a plain name;
/// pattern names are only renameable at the `$var` under the caret.
fn glyph_rename_at(field: &LineField, col: usize, at_base: Option<&str>) -> Option<RenameTarget> {
    if let Some(t) = scan_dollar_ref_at(&field.token, field.col_start, col) {
        return Some(t);
    }
    if field.contains_col(col) && !field.token.contains('$') && !field.token.contains('(') {
        return Some(RenameTarget {
            // The glyph being renamed is the one the name resolves to, so an
            // `@` token renames `foo-bar` and not a glyph spelled `@-bar`.
            name: crate::document::expand_at_name(&field.token, at_base),
            kind: RenameKind::Glyph,
            col_start: field.col_start,
            col_end: field.col_end,
        });
    }
    None
}

fn whole_field_rename(field: &LineField, col: usize, kind: RenameKind) -> Option<RenameTarget> {
    if !field.contains_col(col) {
        return None;
    }
    Some(RenameTarget {
        name: field.token.clone(),
        kind,
        col_start: field.col_start,
        col_end: field.col_end,
    })
}

pub(crate) fn find_renameable_at_caret(
    line: &str,
    col: usize,
    at_base: Option<&str>,
) -> Option<RenameTarget> {
    for f in classify_line(line) {
        let target = match f.role {
            FieldRole::GlyphDef | FieldRole::GlyphRef => glyph_rename_at(&f, col, at_base),
            FieldRole::NamePartsValue => scan_dollar_ref_at(&f.token, f.col_start, col),
            FieldRole::NamePartsDef => whole_field_rename(&f, col, RenameKind::NameParts),
            FieldRole::ColorDef | FieldRole::ColorRef => {
                whole_field_rename(&f, col, RenameKind::Color)
            }
            FieldRole::PointDef => whole_field_rename(&f, col, RenameKind::Point).map(|mut t| {
                t.name = t
                    .name
                    .strip_prefix(['+', '-'])
                    .unwrap_or(&t.name)
                    .to_string();
                t
            }),
            // A face or slice id is renamed from either end: the declaration
            // and every qualifier that names it are the same id, and unlike a
            // remap group the declaration is mandatory, so there is a name to
            // rename rather than a spelling that happens to recur.
            FieldRole::FaceDef | FieldRole::FaceRef => {
                whole_field_rename(&f, col, RenameKind::Face)
            }
            FieldRole::SliceDef | FieldRole::SliceRef => {
                whole_field_rename(&f, col, RenameKind::Slice)
            }
            // A remap group has no single declaration site — every `remap`
            // line that writes a rule names it — so a rename rewrites every
            // appearance and there is no "the" definition to start from. The
            // name never leaves the source, so that is the whole rename.
            FieldRole::RemapGroupDef | FieldRole::RemapGroupRef => {
                whole_field_rename(&f, col, RenameKind::RemapGroup)
            }
            // A feature tag is a registered OpenType tag rather than a name of
            // this font's choosing, so renaming one is retyping it.
            FieldRole::FeatureDef => None,
        };
        if target.is_some() {
            return target;
        }
    }
    None
}

/// A reference whose meaning is written on a *different* token than the one it
/// is spelled on.
enum Capture {
    /// `$-N` — the N-th parenthesized group of the pattern its own item wrote.
    Back(usize),
    /// `$N` — the N-th capturing group of the `exists` search in force, with
    /// `$0` the whole search.
    Search(usize),
}

fn parse_capture(token: &str) -> Option<Capture> {
    let rest = token.strip_prefix('$')?;
    let (back, digits) = match rest.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, rest),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    match (back, n) {
        // `$-0` names nothing: a pattern's groups are numbered from one.
        (true, 0) => None,
        (true, n) => Some(Capture::Back(n)),
        (false, n) => Some(Capture::Search(n)),
    }
}

/// Character offsets of the groups a *name pattern* writes, in the order
/// [`crate::pattern::capture_groups`] numbers them: the outermost parentheses
/// only, since that is what a `$-N` counts.
fn written_group_offsets(text: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut depth = 0usize;
    for (i, c) in text.chars().enumerate() {
        match c {
            '(' => {
                if depth == 0 {
                    offsets.push(i);
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    offsets
}

/// The same for a *regular expression*: every capturing group, numbered by its
/// opening parenthesis — nested ones included, and `(?…)` excluded, which is
/// how the regex crate numbers them and so how [`crate::exists`] does.
fn regex_group_offsets(pattern: &str) -> Vec<usize> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut offsets = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '(' if chars.get(i + 1) != Some(&'?') => offsets.push(i),
            _ => {}
        }
        i += 1;
    }
    offsets
}

/// The tokens of the leading pattern a line writes, as `(column, raw text)` in
/// written order, and whether the groups it binds are named *below* it.
///
/// The two shapes are the ones [`crate::app::search`] tells apart for the same
/// reason: a `glyph` block header's groups are named on the `ref` lines under
/// it, where an alias's and a `map`'s are named further along their own line.
/// `None` is a line that writes no leading pattern at all — a `ref`, an anchor,
/// a pixel row.
fn leading_pattern_spans(line: &str) -> Option<(bool, Vec<(usize, String)>)> {
    let spans = tokenize_with_spans(line).ok()?;
    let raw = |t: &crate::document_io::TokenSpan| {
        let text: String = line
            .chars()
            .skip(t.raw_start)
            .take(t.raw_end - t.raw_start)
            .collect();
        (t.raw_start, text)
    };
    let (keyword, rest) = spans.split_first()?;
    match keyword.value.as_str() {
        "glyph" => {
            let name = rest.first()?;
            let is_alias = rest.get(1).is_some_and(|t| t.value == "=");
            Some((!is_alias, vec![raw(name)]))
        }
        "map" => {
            // The `SLICE :` qualifier first, so what is left is the arity the
            // unqualified form has — as `app::search::line_captures` reads it.
            let rest = match rest.get(1) {
                Some(colon) if colon.value == ":" => &rest[2..],
                _ => rest,
            };
            let rest = match rest.first() {
                Some(g) if g.value == "generate" => &rest[1..],
                _ => rest,
            };
            let eq = rest.iter().position(|t| t.value == "=")?;
            match &rest[..eq] {
                [base] => Some((false, vec![raw(base)])),
                [base, selector] => Some((false, vec![raw(base), raw(selector)])),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Where a `$-N` back-reference or an `exists` `$N` is *written*: the line that
/// binds it and the column of the group it names.
///
/// Both are the same kind of reference — a name whose meaning is stated on
/// another token — and neither is a `name-parts`, so the lookup a `$var` gets
/// would find nothing to go to. A `$-N` names the N-th group of the pattern its
/// own item wrote ([`crate::pattern`]): the `glyph` header above a `ref` line,
/// or the leading pattern of an alias or a `map` on the line itself. A `$N`
/// names the N-th capturing group of the `exists` in force, and `$0` the whole
/// search ([`crate::exists`]).
///
/// Read off the lines rather than the parsed items, like every other navigation
/// question here: the scopes are carried down the file exactly as
/// [`crate::exists::Carry`] and `app::search`'s block captures carry them, so a
/// click answers the same way the Search pane does.
pub(crate) fn find_capture_target(
    lines: &[DocLine],
    from_line: usize,
    token: &str,
) -> Option<(usize, usize)> {
    let capture = parse_capture(token)?;
    let mut carry = crate::exists::Carry::default();
    let mut search_line: Option<usize> = None;
    let mut header_line: Option<usize> = None;
    let mut from_text: Option<&str> = None;
    for (i, line) in lines.iter().enumerate() {
        if i > from_line {
            break;
        }
        let DocLine::Text(text) = line else { continue };
        carry.enter(text);
        // `Armed` is the directive line itself; the pattern stays in force over
        // the item below it and is gone past it.
        match carry {
            crate::exists::Carry::Armed(_) => search_line = Some(i),
            crate::exists::Carry::None => search_line = None,
            _ => {}
        }
        if i == from_line {
            from_text = Some(text);
            break;
        }
        // A `glyph` line that writes groups binds them for the lines under it;
        // any other `glyph` line — an alias, or a header with no groups —
        // clears what the last one left, as `advance_block_captures` does.
        if text.trim_start().starts_with("glyph") {
            header_line = match leading_pattern_spans(text) {
                Some((true, spans)) if spans.iter().any(|(_, t)| t.contains('(')) => Some(i),
                _ => None,
            };
        }
    }
    let from_text = from_text?;

    let nth = |spans: &[(usize, String)], offsets: fn(&str) -> Vec<usize>, n: usize| {
        let mut seen = 0usize;
        for (col, text) in spans {
            for off in offsets(text) {
                seen += 1;
                if seen == n {
                    return Some(col + off);
                }
            }
        }
        None
    };

    match capture {
        Capture::Search(n) => {
            let line = search_line?;
            let text = lines.get(line)?.as_text()?;
            let spans = tokenize_with_spans(text).ok()?;
            let pattern = spans.get(1)?;
            let raw: String = text
                .chars()
                .skip(pattern.raw_start)
                .take(pattern.raw_end - pattern.raw_start)
                .collect();
            // `$0` is the match itself, so it lands on the search rather than
            // on any one group of it.
            if n == 0 {
                return Some((line, pattern.raw_start));
            }
            let col = nth(&[(pattern.raw_start, raw)], regex_group_offsets, n)?;
            Some((line, col))
        }
        Capture::Back(n) => {
            // A line writing its own leading pattern binds its own groups; a
            // `ref` line writes none and names the header's.
            let (line, spans) = match leading_pattern_spans(from_text) {
                Some((false, spans)) => (from_line, spans),
                _ => {
                    let line = header_line?;
                    let (_, spans) = leading_pattern_spans(lines.get(line)?.as_text()?)?;
                    (line, spans)
                }
            };
            let col = nth(&spans, written_group_offsets, n)?;
            Some((line, col))
        }
    }
}

/// The line in `lines` that declares `name`, if this document declares it.
///
/// `parts` is only read for the glyph case, where a block's name may be a
/// pattern standing for every name it expands to — see [`pattern_denotes`].
pub fn find_link_target_in_doc(
    lines: &[DocLine],
    name: &str,
    kind: &LinkTargetKind,
    parts: &NamePartsMap,
) -> Option<usize> {
    match kind {
        LinkTargetKind::Glyph => {
            let mut exists = crate::exists::Carry::default();
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    exists.enter(s);
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "glyph")
                        && tokens.get(1).is_some_and(|t| {
                            t == name
                                || pattern_denotes(t, true, name, parts, exists.pattern(), &[])
                        })
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::NameParts => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "name-parts")
                        && tokens.get(1).is_some_and(|t| t == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::Remap => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "remap")
                        && let Some(first) = tokens.get(1)
                        && (first == name || first.trim_end_matches(':') == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::Color => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "color")
                        && tokens.get(1).is_some_and(|t| t == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::Face | LinkTargetKind::Slice => {
            let keyword = if *kind == LinkTargetKind::Face {
                "face"
            } else {
                "slice"
            };
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == keyword)
                        && tokens.get(1).is_some_and(|t| t == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        // Neither has a declaration site to go to; both only ever search.
        LinkTargetKind::Anchor | LinkTargetKind::Feature => None,
    }
}

#[cfg(test)]
mod rename_detection_tests {
    use super::*;

    #[test]
    fn glyph_header_name() {
        let t = find_renameable_at_caret("glyph foo 8 16", 6, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_header_name_at_end() {
        // col 9 = right after "foo" (col_end), should still match
        let t = find_renameable_at_caret("glyph foo 8 16", 9, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_header_on_dimensions() {
        assert!(find_renameable_at_caret("glyph foo 8 16", 10, None).is_none());
    }

    #[test]
    fn glyph_alias_def_name() {
        let t = find_renameable_at_caret("glyph foo = bar", 6, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_alias_target() {
        let t = find_renameable_at_caret("glyph foo = bar", 12, None).unwrap();
        assert_eq!(t.name, "bar");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    /// Flags before the `=` no longer parse (an alias takes none), but the
    /// editor reads the line as text and must keep working on one being typed
    /// or half-migrated.
    fn glyph_alias_target_survives_stray_flags() {
        let t = find_renameable_at_caret("glyph foo advance 8 = bar", 23, None).unwrap();
        assert_eq!(t.name, "bar");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn feature_for_script_links_remap_group() {
        let links = extract_line_links("feature ljmo for hang : hangul-ljmo", None);
        // The group is a reference and navigates; the tag names nothing else,
        // so it only ever searches.
        assert_eq!(
            links
                .iter()
                .map(|l| (l.target.as_str(), l.kind, l.is_def))
                .collect::<Vec<_>>(),
            vec![
                ("ljmo", LinkTargetKind::Feature, true),
                ("hangul-ljmo", LinkTargetKind::Remap, false),
            ],
        );
    }

    /// Every definition is clickable too — with nothing to go to, the click
    /// asks for the name's appearances instead.
    #[test]
    fn definitions_are_links_marked_as_definitions() {
        for (line, target, kind) in [
            ("glyph foo 8 16", "foo", LinkTargetKind::Glyph),
            ("name-parts $init = a b", "$init", LinkTargetKind::NameParts),
            ("color red = #ff0000", "red", LinkTargetKind::Color),
            ("remap liga : a -> b", "liga", LinkTargetKind::Remap),
        ] {
            let links = extract_line_links(line, None);
            let found = links
                .iter()
                .find(|l| l.target == target)
                .unwrap_or_else(|| panic!("no link for {target} in {line:?}"));
            assert_eq!(found.kind, kind, "{line:?}");
            assert!(found.is_def, "{line:?}");
        }
    }

    /// An anchor is one anchor whichever sign it carries, so the link drops it.
    #[test]
    fn an_anchor_link_drops_its_sign() {
        for line in ["anchor +above 4 1", "anchor -above 2 1"] {
            let links = extract_line_links(line, None);
            assert_eq!(links.len(), 1, "{line:?}");
            assert_eq!(links[0].target, "above");
            assert_eq!(links[0].kind, LinkTargetKind::Anchor);
            assert!(links[0].is_def);
        }
    }

    /// A pattern name is not a name anything can refer to, so only the `$var`s
    /// inside it are links.
    #[test]
    fn a_pattern_glyph_definition_links_only_its_variables() {
        let links = extract_line_links("glyph hangul-($init)-l 8 16", None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "$init");
        assert!(!links[0].is_def);
    }

    #[test]
    fn ref_name() {
        let t = find_renameable_at_caret("ref latin-a 0 0", 4, None).unwrap();
        assert_eq!(t.name, "latin-a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn map_name() {
        let t = find_renameable_at_caret("map A = latin-a", 8, None).unwrap();
        assert_eq!(t.name, "latin-a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn name_parts_def() {
        let t = find_renameable_at_caret("name-parts $init = a b c", 11, None).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn name_parts_ref_in_values() {
        let t = find_renameable_at_caret("name-parts $combo = $init $final", 20, None).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn dollar_var_in_glyph_header() {
        let t = find_renameable_at_caret("glyph hangul-($init)-l 8 16", 14, None).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn pattern_glyph_non_var_part() {
        // Caret on the non-$var part of a pattern name — not renameable
        assert!(find_renameable_at_caret("glyph hangul-($init)-l 8 16", 6, None).is_none());
    }

    #[test]
    fn point_plus() {
        let t = find_renameable_at_caret("anchor +above 4 1", 7, None).unwrap();
        assert_eq!(t.name, "above");
        assert_eq!(t.kind, RenameKind::Point);
    }

    #[test]
    fn point_minus() {
        let t = find_renameable_at_caret("anchor -above 2 1", 8, None).unwrap();
        assert_eq!(t.name, "above");
        assert_eq!(t.kind, RenameKind::Point);
    }

    #[test]
    fn point_on_coords() {
        assert!(find_renameable_at_caret("anchor +above 4 1", 15, None).is_none());
    }

    #[test]
    fn remap_token() {
        let t = find_renameable_at_caret("remap liga : a -> b", 13, None).unwrap();
        assert_eq!(t.name, "a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn exclude_from_sample() {
        let t = find_renameable_at_caret("exclude-from-sample foo", 20, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn empty_line() {
        assert!(find_renameable_at_caret("", 0, None).is_none());
    }

    #[test]
    fn comment_line() {
        assert!(find_renameable_at_caret("# some comment", 2, None).is_none());
    }

    #[test]
    fn color_def_name() {
        let t = find_renameable_at_caret("color red = #ff0000", 6, None).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn color_def_value_hex_is_renameable() {
        // Hex values are not color name references, but they are still renameable
        // (a hex value is not a color name, so no rename target)
        assert!(find_renameable_at_caret("color red = #ff0000", 12, None).is_none());
    }

    #[test]
    fn color_def_value_name_ref() {
        let t = find_renameable_at_caret("color light-red = red", 18, None).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn ref_fill_color_name() {
        let t = find_renameable_at_caret("ref foo 0 0 fill red", 17, None).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn ref_fill_fg_not_renameable() {
        assert!(find_renameable_at_caret("ref foo 0 0 fill fg", 17, None).is_none());
    }

    #[test]
    fn ref_fill_hex_not_renameable() {
        assert!(find_renameable_at_caret("ref foo 0 0 fill #ff0000", 17, None).is_none());
    }

    #[test]
    fn color_links_value_name() {
        let links = extract_line_links("color light-red = red", None);
        assert_eq!(
            links
                .iter()
                .map(|l| (l.target.as_str(), l.is_def))
                .collect::<Vec<_>>(),
            vec![("light-red", true), ("red", false)],
        );
        assert!(links.iter().all(|l| l.kind == LinkTargetKind::Color));
    }

    #[test]
    fn color_links_value_hex_no_link() {
        // The hex value is not a name; only the color being defined is.
        let links = extract_line_links("color red = #ff0000", None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "red");
        assert!(links[0].is_def);
    }

    #[test]
    fn ref_fill_links_color_name() {
        let links = extract_line_links("ref foo 0 0 fill red", None);
        // Should have glyph link for 'foo' AND color link for 'red'
        assert!(
            links
                .iter()
                .any(|l| l.target == "red" && matches!(l.kind, LinkTargetKind::Color))
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == "foo" && matches!(l.kind, LinkTargetKind::Glyph))
        );
    }

    #[test]
    fn ref_fill_links_fg_no_color_link() {
        let links = extract_line_links("ref foo 0 0 fill fg", None);
        assert!(
            !links
                .iter()
                .any(|l| matches!(l.kind, LinkTargetKind::Color))
        );
    }

    #[test]
    fn ref_fill_links_hex_no_color_link() {
        let links = extract_line_links("ref foo 0 0 fill #ff0000", None);
        assert!(
            !links
                .iter()
                .any(|l| matches!(l.kind, LinkTargetKind::Color))
        );
    }

    #[test]
    fn assert_same_links_glyph_names() {
        let links = extract_line_links("assert same foo bar", None);
        assert_eq!(links.len(), 2);
        assert!(
            links
                .iter()
                .any(|l| l.target == "foo" && matches!(l.kind, LinkTargetKind::Glyph))
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == "bar" && matches!(l.kind, LinkTargetKind::Glyph))
        );
    }

    #[test]
    fn assert_distinct_links_glyph_names() {
        let links = extract_line_links("assert distinct a b c", None);
        assert_eq!(links.len(), 3);
        assert!(links.iter().any(|l| l.target == "a"));
        assert!(links.iter().any(|l| l.target == "b"));
        assert!(links.iter().any(|l| l.target == "c"));
    }

    #[test]
    fn assert_same_comment_not_linked() {
        let links = extract_line_links("assert same foo bar // not a glyph", None);
        let glyph_names: Vec<&str> = links
            .iter()
            .filter(|l| matches!(l.kind, LinkTargetKind::Glyph))
            .map(|l| l.target.as_str())
            .collect();
        assert_eq!(glyph_names.len(), 2);
        assert!(glyph_names.contains(&"foo"));
        assert!(glyph_names.contains(&"bar"));
    }

    #[test]
    fn assert_shape_links_glyph_names() {
        let links = extract_line_links("assert shape AB : a-upper : b-upper", None);
        assert!(
            links
                .iter()
                .any(|l| l.target == "a-upper" && matches!(l.kind, LinkTargetKind::Glyph))
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == "b-upper" && matches!(l.kind, LinkTargetKind::Glyph))
        );
    }

    #[test]
    fn assert_same_rename_glyph() {
        let t = find_renameable_at_caret("assert same foo bar", 12, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_distinct_rename_glyph() {
        let t = find_renameable_at_caret("assert distinct abc def", 16, None).unwrap();
        assert_eq!(t.name, "abc");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_same_rename_not_on_keyword() {
        assert!(find_renameable_at_caret("assert same foo bar", 0, None).is_none());
        assert!(find_renameable_at_caret("assert same foo bar", 7, None).is_none());
    }

    #[test]
    fn assert_shape_rename_glyph() {
        let t = find_renameable_at_caret("assert shape AB : a-upper : b-upper", 18, None).unwrap();
        assert_eq!(t.name, "a-upper");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_rename_not_in_comment() {
        assert!(find_renameable_at_caret("assert same foo bar // comment", 23, None).is_none());
    }

    /// A face declares its id and refers to the slices it includes; a slice
    /// declares its id and refers to the ones it unions.
    /// F2 works from either end of a face, slice or remap-group id — the
    /// declaration and every use are the same name.
    #[test]
    fn faces_slices_and_remap_groups_are_renameable() {
        for (line, col, name, kind) in [
            ("face term : narrow", 6, "term", RenameKind::Face),
            ("face term : narrow", 13, "narrow", RenameKind::Slice),
            ("slice both = narrow wide", 7, "both", RenameKind::Slice),
            ("meta term : family Unison", 6, "term", RenameKind::Face),
            ("map narrow : A = latin-a", 6, "narrow", RenameKind::Slice),
            (
                "assert shape AB for narrow : a-b",
                22,
                "narrow",
                RenameKind::Slice,
            ),
            ("remap liga : a -> b", 7, "liga", RenameKind::RemapGroup),
            (
                "remap group liga after flag",
                13,
                "liga",
                RenameKind::RemapGroup,
            ),
            (
                "remap group liga after flag",
                24,
                "flag",
                RenameKind::RemapGroup,
            ),
            (
                "feature dlig for latn : liga",
                25,
                "liga",
                RenameKind::RemapGroup,
            ),
        ] {
            let t = find_renameable_at_caret(line, col, None)
                .unwrap_or_else(|| panic!("nothing renameable at {col} in {line:?}"));
            assert_eq!(t.name, name, "{line:?}");
            assert_eq!(t.kind, kind, "{line:?}");
        }
        // An OpenType tag is not this font's name to change.
        assert!(find_renameable_at_caret("feature dlig for latn : liga", 9, None).is_none());
        // `*` is "every face", not a face.
        assert!(find_renameable_at_caret("meta * : family Unison", 5, None).is_none());
    }

    #[test]
    fn face_and_slice_lines_link_both_ways() {
        assert_eq!(
            extract_line_links("face term : narrow", None)
                .iter()
                .map(|l| (l.target.as_str(), l.kind, l.is_def))
                .collect::<Vec<_>>(),
            vec![
                ("term", LinkTargetKind::Face, true),
                ("narrow", LinkTargetKind::Slice, false)
            ],
        );
        assert_eq!(
            extract_line_links("slice both = narrow wide", None)
                .iter()
                .map(|l| (l.target.as_str(), l.kind, l.is_def))
                .collect::<Vec<_>>(),
            vec![
                ("both", LinkTargetKind::Slice, true),
                ("narrow", LinkTargetKind::Slice, false),
                ("wide", LinkTargetKind::Slice, false),
            ],
        );
    }

    /// The qualifiers the new directives added are references too — a `map`
    /// under a slice links to the slice as well as to its glyph.
    #[test]
    fn qualifiers_link_to_their_slice_or_face() {
        for (line, target, kind) in [
            ("map narrow : A = latin-a", "narrow", LinkTargetKind::Slice),
            (
                "feature wide : liga for latn : eq-liga",
                "wide",
                LinkTargetKind::Slice,
            ),
            (
                "meta term : family Unison Term",
                "term",
                LinkTargetKind::Face,
            ),
            (
                "assert shape AB for narrow : a-b",
                "narrow",
                LinkTargetKind::Slice,
            ),
        ] {
            let links = extract_line_links(line, None);
            let found = links
                .iter()
                .find(|l| l.target == target)
                .unwrap_or_else(|| panic!("no link for {target} in {line:?}"));
            assert_eq!(found.kind, kind, "{line:?}");
            assert!(!found.is_def, "{line:?}");
        }
        // The glyph on a qualified `map` is still a link of its own.
        assert!(
            extract_line_links("map narrow : A = latin-a", None)
                .iter()
                .any(|l| l.target == "latin-a" && l.kind == LinkTargetKind::Glyph)
        );
    }

    /// The declaration a qualifier points at is what a Ctrl/Cmd+click goes to.
    #[test]
    fn face_and_slice_declarations_are_found_in_a_document() {
        let lines: Vec<DocLine> = ["slice narrow", "", "face term : narrow"]
            .iter()
            .map(|s| DocLine::Text(s.to_string()))
            .collect();
        assert_eq!(
            find_link_target_in_doc(
                &lines,
                "narrow",
                &LinkTargetKind::Slice,
                &NamePartsMap::new()
            ),
            Some(0),
        );
        assert_eq!(
            find_link_target_in_doc(&lines, "term", &LinkTargetKind::Face, &NamePartsMap::new()),
            Some(2),
        );
        // The two ids live in different namespaces and never cross.
        assert_eq!(
            find_link_target_in_doc(
                &lines,
                "narrow",
                &LinkTargetKind::Face,
                &NamePartsMap::new()
            ),
            None,
        );
    }

    /// A glyph block whose name is written as a *pattern* declares every name
    /// the pattern expands to, so a click on one of those names — from the
    /// specimen, or from a `ref` naming it — goes to that line.
    #[test]
    fn a_pattern_glyph_definition_is_found_in_a_document() {
        let lines: Vec<DocLine> = ["glyph latin-a 8 16", "glyph han-($#4e00..9fff) 16 16"]
            .iter()
            .map(|s| DocLine::Text(s.to_string()))
            .collect();
        let parts = NamePartsMap::new();
        assert_eq!(
            find_link_target_in_doc(&lines, "han-4e01", &LinkTargetKind::Glyph, &parts),
            Some(1),
        );
        // A name the pattern does not cover is still not declared here.
        assert_eq!(
            find_link_target_in_doc(&lines, "han-3400", &LinkTargetKind::Glyph, &parts),
            None,
        );
    }

    /// The same, for a block whose names come from an `exists` above it: the
    /// header alone says nothing about `han-4e00`, so navigation has to read
    /// the directive with it.
    #[test]
    fn an_exists_block_is_found_as_the_definition_of_what_it_declares() {
        let lines: Vec<DocLine> = [
            "glyph latin-a 8 16",
            "exists han-([0-9a-f]{4,5}):15x16",
            "glyph han-($1) 16 16 advance 16",
            "ref ($0) 1 0",
        ]
        .iter()
        .map(|s| DocLine::Text(s.to_string()))
        .collect();
        let parts = NamePartsMap::new();
        assert_eq!(
            find_link_target_in_doc(&lines, "han-4e00", &LinkTargetKind::Glyph, &parts),
            Some(2),
        );
        // The variant the search reads is declared elsewhere, not here.
        assert_eq!(
            find_link_target_in_doc(&lines, "han-4e00:15x16", &LinkTargetKind::Glyph, &parts),
            None,
        );
        // And a name outside what the capture can hold is not declared here.
        assert_eq!(
            find_link_target_in_doc(&lines, "han-zzzz", &LinkTargetKind::Glyph, &parts),
            None,
        );
    }

    #[test]
    fn exclude_from_sample_links_glyph_name() {
        let links = extract_line_links("exclude-from-sample foo", None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "foo");
        assert!(matches!(links[0].kind, LinkTargetKind::Glyph));
    }

    #[test]
    fn assume_unused_links_glyph_names() {
        let links = extract_line_links("assume unused foo bar", None);
        assert!(links.iter().any(|l| l.target == "foo"));
        assert!(links.iter().any(|l| l.target == "bar"));
    }

    /// An `@` name links to the glyph it expands to — the one that exists —
    /// and renaming from it renames that glyph, not a name spelled `@-bar`.
    #[test]
    fn an_at_name_links_to_what_it_expands_to() {
        let links = extract_line_links("ref @-bar", Some("foo"));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "foo-bar");
        assert!(!links[0].is_def);

        let links = extract_line_links("glyph @-bar", Some("foo"));
        assert_eq!(links[0].target, "foo-bar");
        assert!(links[0].is_def);

        let t = find_renameable_at_caret("ref @-bar", 6, Some("foo")).unwrap();
        assert_eq!(t.name, "foo-bar");
        assert_eq!(t.kind, RenameKind::Glyph);

        // With no base the `@` stands for nothing, and the token is left to be
        // reported as the invalid name it is.
        assert_eq!(extract_line_links("ref @-bar", None)[0].target, "@-bar");
    }
}

/// Where a `$-N` and a `$N` are written — the lines the click on one jumps to.
/// The scenarios these stand for are in
/// [`crate::editor::view_tests::links`](crate::editor::view_tests).
#[cfg(test)]
mod capture_target_tests {
    use super::*;

    fn doc(source: &str) -> Vec<DocLine> {
        crate::document_io::parse_doclines(source)
    }

    /// Index of the line starting with `prefix`. A declared box gives a glyph
    /// block a `Grid` line of its own, so the indices are not the source's.
    #[track_caller]
    fn at(lines: &[DocLine], prefix: &str) -> usize {
        lines
            .iter()
            .position(|l| matches!(l, DocLine::Text(s) if s.starts_with(prefix)))
            .unwrap_or_else(|| panic!("no line starting with {prefix:?}"))
    }

    /// The header above the line, and the group counted from one.
    #[test]
    fn a_ref_names_the_header_above_it() {
        let lines = doc("glyph a-(x|y)-(1|2) 2 2\nref b-($-2) 0 0\n");
        let (header, r) = (at(&lines, "glyph"), at(&lines, "ref"));
        assert_eq!(find_capture_target(&lines, r, "$-1"), Some((header, 8)));
        assert_eq!(find_capture_target(&lines, r, "$-2"), Some((header, 14)));
        // A group the header does not have is nowhere to go.
        assert_eq!(find_capture_target(&lines, r, "$-3"), None);
    }

    /// An alias and a `map` write their pattern and name it again on the same
    /// line, so neither reaches for a header — and a `map`'s two halves are
    /// numbered in written order.
    #[test]
    fn a_line_that_binds_its_own_groups_stays_on_it() {
        let lines = doc("glyph a-(x|y) 2 2\nglyph b-(p|q) = c-($-1)\n");
        let alias = at(&lines, "glyph b-");
        assert_eq!(find_capture_target(&lines, alias, "$-1"), Some((alias, 8)));

        let lines = doc("map U+(4e00|4e01) U+E010(0|1) = han-($-1).($-2)\n");
        assert_eq!(find_capture_target(&lines, 0, "$-1"), Some((0, 6)));
        assert_eq!(find_capture_target(&lines, 0, "$-2"), Some((0, 24)));
    }

    /// A header with no groups clears the one above it rather than leaving a
    /// stale binding behind.
    #[test]
    fn a_header_without_groups_binds_nothing() {
        let lines = doc("glyph a-(x|y) 2 2\nglyph plain 2 2\nref b-($-1) 0 0\n");
        assert_eq!(find_capture_target(&lines, at(&lines, "ref"), "$-1"), None);
    }

    /// A search governs the item below it and nothing past it; `$0` is the
    /// search itself, and a nested group is numbered by its own parenthesis.
    #[test]
    fn a_search_capture_names_the_exists_line() {
        let lines = doc(
            "exists han-(([0-9a-f]{4}))-(k):2x2\nglyph han-($1)-($3) 2 2\nref ($0) 0 0\nglyph other 2 2\nref ($0) 0 0\n",
        );
        let ex = at(&lines, "exists");
        let header = at(&lines, "glyph han-");
        let r = at(&lines, "ref");
        assert_eq!(find_capture_target(&lines, header, "$0"), Some((ex, 7)));
        assert_eq!(find_capture_target(&lines, header, "$1"), Some((ex, 11)));
        assert_eq!(find_capture_target(&lines, header, "$2"), Some((ex, 12)));
        assert_eq!(find_capture_target(&lines, r, "$3"), Some((ex, 27)));
        // Past the block the search governs there is nothing to name.
        let past = lines.len() - 1;
        assert_eq!(find_capture_target(&lines, past, "$0"), None);
    }

    /// Neither spelling is a name-parts reference, and a plain `$var` is not
    /// one of these.
    #[test]
    fn a_name_part_is_not_a_capture() {
        let lines = doc("glyph a-(x|y) 2 2\nref b-($foo) 0 0\n");
        let r = at(&lines, "ref");
        assert_eq!(find_capture_target(&lines, r, "$foo"), None);
        assert_eq!(find_capture_target(&lines, r, "$-0"), None);
    }
}

#[cfg(test)]
mod comment_link_tests {
    use super::*;
    use std::collections::HashSet;

    fn links(line: &str, known: &[&str]) -> Vec<(usize, usize, String)> {
        let set: HashSet<&str> = known.iter().copied().collect();
        let mut out = Vec::new();
        extract_comment_links(line, &|n| set.contains(n), &mut out);
        out.iter()
            .map(|l| (l.col_start, l.col_end, l.target.clone()))
            .collect()
    }

    #[test]
    fn only_the_words_that_name_a_glyph_link() {
        let line = "ref a 0 0 // like a, unlike zzz";
        assert_eq!(
            links(line, &["a", "b"]),
            vec![(
                line.rfind(" a,").unwrap() + 1,
                line.find("a,").unwrap() + 1,
                "a".to_string()
            )]
        );
    }

    #[test]
    fn a_word_ends_where_a_name_character_does_not_continue() {
        // Finer than whitespace on both sides: the Korean particle is no part
        // of the name, and neither is the comma.
        let line = "// han-4e00을, han-4e01";
        let got = links(line, &["han-4e00", "han-4e01"]);
        assert_eq!(
            got.iter().map(|(_, _, n)| n.as_str()).collect::<Vec<_>>(),
            ["han-4e00", "han-4e01"]
        );
        assert_eq!(got[0].0, 3);
        assert_eq!(got[0].1, 11);
    }

    #[test]
    fn a_line_with_no_comment_contributes_nothing() {
        assert!(links("ref a 0 0", &["a"]).is_empty());
        // A `//` that is not at a token start is not a comment, and the
        // tokenizer already decided that — this only has to agree with it.
        assert!(links("map a = a//b", &["a"]).is_empty());
    }

    #[test]
    fn a_trailing_stop_is_part_of_the_word() {
        // `.` is a glyph-name character (`num.1`), so the run does not stop at
        // one: `a.` names nothing and links nowhere.
        assert!(links("// see a.", &["a"]).is_empty());
        assert_eq!(links("// see num.1", &["num.1"]).len(), 1);
    }
}
