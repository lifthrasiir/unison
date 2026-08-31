//! Serializing a [`DocumentItem`] back to the `.unf` line (or lines) it was
//! written as. The parser is in [`crate::document_io`], which is also the
//! reference for the syntax this puts back.

use super::*;

#[cfg(any(feature = "editor", test))]
use crate::document_io::comment_suffix as serialize_comment_suffix;

/// `SLICE : ` in front of a directive body, or nothing for the base slice.
#[cfg(any(feature = "editor", test))]
fn serialize_slice_prefix(slices: &[String]) -> String {
    crate::document_io::slice_prefix(slices)
}

impl DocumentItem {
    /// Parse a structured directive from pre-tokenized tokens (the line's
    /// `// …` comment already split off and passed as `comment`).
    /// The first token is the keyword ("name-parts", "remap", or "feature").
    pub fn parse_directive(tokens: &[String], comment: Option<String>) -> DocumentItem {
        if tokens.is_empty() {
            return DocumentItem::Directive(String::new());
        }
        match tokens[0].as_str() {
            "name-parts" => {
                let (slices, rest) = Self::split_slice_qualifier(&tokens[1..]);
                if rest.len() >= 3 && rest[1] == "=" {
                    return DocumentItem::NameParts {
                        slices,
                        name: rest[0].clone(),
                        values: rest[2..].to_vec(),
                        comment,
                    };
                }
            }
            "assert" => {
                if tokens.get(1).is_some_and(|t| t == "shape")
                    && let Some(item) = Self::parse_assert_shape(&tokens[2..], comment.clone())
                {
                    return item;
                }
                match tokens.get(1).map(|s| s.as_str()) {
                    Some("same") if tokens.len() >= 4 => {
                        return DocumentItem::AssertSame {
                            names: tokens[2..].to_vec(),
                            comment,
                        };
                    }
                    Some("distinct") if tokens.len() >= 4 => {
                        return DocumentItem::AssertDistinct {
                            names: tokens[2..].to_vec(),
                            comment,
                        };
                    }
                    _ => {}
                }
            }
            "prop" => {
                if let Some(item) = Self::parse_prop(&tokens[1..], comment.clone()) {
                    return item;
                }
            }
            "remap" => {
                // A rule always has a colon before its arrow, so the two forms
                // never compete — even for a group that is literally named
                // `group`, whose rules read `remap group : a -> b`.
                if let Some(item) = Self::parse_remap(&tokens[1..], comment.clone()) {
                    return item;
                }
                if let Some(item) = Self::parse_remap_group(&tokens[1..], comment.clone()) {
                    return item;
                }
            }
            "face" | "slice" => {
                // `face F [: S...]` and `slice S [= S...]`. The separator
                // differs because the two mean different things: a face
                // *includes* slices, a slice *is* the union of others.
                let rest = &tokens[1..];
                let sep = if tokens[0] == "face" { ":" } else { "=" };
                if let Some(id) = rest.first() {
                    let refs: Vec<String> = match rest.get(1) {
                        None => Vec::new(),
                        Some(t) if t == sep && rest.len() > 2 => rest[2..].to_vec(),
                        // Anything else is malformed; fall through to the raw
                        // line rather than half-reading it.
                        Some(_) => return Self::unrecognized(tokens, comment),
                    };
                    if tokens[0] == "face" {
                        return DocumentItem::Face {
                            id: id.clone(),
                            slices: refs,
                            comment,
                        };
                    }
                    return DocumentItem::Slice {
                        id: id.clone(),
                        inherits: refs,
                        comment,
                    };
                }
            }
            "feature" => {
                let (slices, rest) = Self::split_slice_qualifier(&tokens[1..]);
                // feature NAME for SCRIPT... : REMAP_GROUP
                // feature NAME for SCRIPT... : anchor ANCHOR_NAME
                if let Some(for_pos) = rest.iter().position(|t| t == "for")
                    && let Some(colon_pos) = rest.iter().position(|t| t == ":")
                    && for_pos == 1
                    && colon_pos > 2
                    && colon_pos + 1 < rest.len()
                {
                    if rest.get(colon_pos + 1).is_some_and(|t| t == "anchor")
                        && colon_pos + 2 < rest.len()
                    {
                        // `align XX` is optional and only ever follows the
                        // anchor name; an unreadable one leaves the default
                        // rather than dropping the whole declaration, and
                        // `issues::anchors` is what says so.
                        let align = match &rest[colon_pos + 3..] {
                            [keyword, token] if keyword == "align" => {
                                AnchorAlign::from_token(token).unwrap_or_default()
                            }
                            _ => AnchorAlign::default(),
                        };
                        return DocumentItem::FeatureAnchor {
                            slices,
                            name: rest[0].clone(),
                            scripts: rest[2..colon_pos].to_vec(),
                            anchor: rest[colon_pos + 2].clone(),
                            align,
                            comment,
                        };
                    }
                    return DocumentItem::Feature {
                        slices,
                        name: rest[0].clone(),
                        scripts: rest[2..colon_pos].to_vec(),
                        remap_group: rest[colon_pos + 1].clone(),
                        comment,
                    };
                }
            }
            _ => {}
        }
        Self::unrecognized(tokens, comment)
    }

    /// `prop ...`, in either of its two forms — the tokens after the keyword.
    ///
    /// `None` for anything malformed, which the caller keeps as raw text and
    /// [`crate::issues`] reports. The two forms are told apart by the first
    /// token being `block`, which no character spelling can be (a name is one
    /// character or a `U+…` form), so a block never shadows a character.
    ///
    /// The property keywords may come in any order and any subset; a keyword
    /// with no value, an unknown one, or a `ccc` that is not a `u8` makes the
    /// whole line malformed rather than half-read.
    fn parse_prop(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        if tokens.first().is_some_and(|t| t == "block") {
            if tokens.len() != 4 || tokens[2] != "=" {
                return None;
            }
            let (start, end) = crate::ucd::parse_block_range(&tokens[3])?;
            return Some(DocumentItem::PropBlock {
                name: tokens[1].clone(),
                start,
                end,
                comment,
            });
        }

        let char_repr = tokens.first()?.clone();
        let mut idx = 1;
        // `= NAME` is optional, and so is every property — but a line that
        // states neither is not a `prop` line at all.
        let name = if tokens.get(idx).is_some_and(|t| t == "=") {
            idx += 2;
            Some(tokens.get(idx - 1)?.clone())
        } else {
            None
        };

        let mut values = crate::ucd::CharPropValues::default();
        while idx < tokens.len() {
            let value = tokens.get(idx + 1)?;
            match tokens[idx].as_str() {
                "gc" => values.gc = Some(value.clone()),
                "ccc" => values.ccc = Some(value.parse().ok()?),
                "eaw" => values.eaw = Some(value.clone()),
                _ => return None,
            }
            idx += 2;
        }
        if name.is_none() && values.is_empty() {
            return None;
        }

        Some(DocumentItem::PropChar {
            char_repr,
            name,
            values,
            comment,
        })
    }

    /// Malformed: keep the line as raw text, comment included, so nothing is
    /// lost on the way back out.
    fn unrecognized(tokens: &[String], comment: Option<String>) -> DocumentItem {
        let quoted: Vec<String> = tokens
            .iter()
            .map(|t| crate::document_io::quote_token(t))
            .collect();
        let comment = match comment {
            Some(c) => format!(" // {c}"),
            None => String::new(),
        };
        DocumentItem::Directive(format!("{}{}", quoted.join(" "), comment))
    }

    /// Split a leading `SLICE[|SLICE...] :` qualifier off a directive body.
    ///
    /// Told from the body by the *second* token being a bare `:`, which no name
    /// or value can be. That is what keeps `map : = colon` — a perfectly good
    /// mapping of U+003A — from reading as a qualifier, and it still allows
    /// `map wide : : = colon` to qualify one.
    ///
    /// The qualifier is *one* token: a slice id may not contain `|`, so
    /// `wide|narrow` is unambiguously a list of two. Listing slices states the
    /// line once per slice rather than once — the slices are an outer loop
    /// around name expansion, not another alternation group folded into it; see
    /// [`crate::pattern`].
    pub(crate) fn split_slice_qualifier(tokens: &[String]) -> (Vec<String>, &[String]) {
        match Self::split_qualifier_token(tokens) {
            (Some(q), rest) => (q.split('|').map(str::to_string).collect(), rest),
            (None, rest) => (Vec::new(), rest),
        }
    }

    /// The qualifier as the single token it is written as. `meta FACE :` reads
    /// it this way: a face scope is one id, never a list.
    pub(crate) fn split_qualifier_token(tokens: &[String]) -> (Option<String>, &[String]) {
        if tokens.len() >= 2 && tokens[1] == ":" && tokens[0] != ":" {
            (Some(tokens[0].clone()), &tokens[2..])
        } else {
            (None, tokens)
        }
    }

    /// The slices this item is qualified with; empty for the base slice and for
    /// every item that takes no qualifier.
    ///
    /// `assert shape` is deliberately not here: its `for SLICE...` list means
    /// *all of these*, while a qualifier means *each of these*.
    pub fn slice_qualifier(&self) -> &[String] {
        match self {
            DocumentItem::Map { slices, .. }
            | DocumentItem::MapDecomposed { slices, .. }
            | DocumentItem::Feature { slices, .. }
            | DocumentItem::FeatureAnchor { slices, .. }
            | DocumentItem::NameParts { slices, .. } => slices,
            _ => &[],
        }
    }

    fn parse_remap(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        let arrow_pos = tokens.iter().position(|t| t == "->")?;

        let colon_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| t.as_str() == ":")
            .map(|(i, _)| i)
            .collect();

        let first = tokens.first()?;
        let (feature, first_colon_after_feature) = if first.ends_with(':') && first.len() > 1 {
            (first.trim_end_matches(':').to_string(), 1)
        } else {
            let fc = colon_positions.iter().copied().find(|&p| p < arrow_pos)?;
            // Only the group name may precede that colon. Everything between
            // the two used to be skipped over silently, so a typo in the group
            // name half of the line built a rule nobody had written.
            if fc != 1 {
                return None;
            }
            (first.clone(), fc + 1)
        };

        let last_colon_before_arrow = colon_positions
            .iter()
            .copied()
            .rfind(|&p| p >= first_colon_after_feature && p < arrow_pos);

        let (lookbehind, source_start) = if let Some(lc) = last_colon_before_arrow {
            let lb: Vec<String> = tokens[first_colon_after_feature..lc].to_vec();
            (lb, lc + 1)
        } else {
            (Vec::new(), first_colon_after_feature)
        };

        let source = tokens[source_start..arrow_pos].to_vec();

        let after_arrow = arrow_pos + 1;
        let lookahead_colon = colon_positions.iter().copied().find(|&p| p > arrow_pos);

        let (target, lookahead) = if let Some(lc) = lookahead_colon {
            let target = tokens[after_arrow..lc].to_vec();
            let la: Vec<String> = tokens[lc + 1..].to_vec();
            (target, la)
        } else {
            (tokens[after_arrow..].to_vec(), Vec::new())
        };

        Some(DocumentItem::Remap {
            feature,
            lookbehind,
            source,
            target,
            lookahead,
            comment,
        })
    }

    /// `group NAME [reversed] [after GROUP]...`, the tokens after `remap`.
    /// Every flag is checked rather than skipped: a line that half-parses would
    /// silently lose an ordering constraint, which shows up only as a
    /// mis-shaped glyph much later.
    fn parse_remap_group(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        if tokens.first()? != "group" {
            return None;
        }
        let name = tokens.get(1)?.clone();
        if name == "reversed" || name == "after" {
            return None;
        }

        let mut reversed = false;
        let mut after: Vec<String> = Vec::new();
        let mut i = 2;
        while i < tokens.len() {
            match tokens[i].as_str() {
                "reversed" if !reversed => {
                    reversed = true;
                    i += 1;
                }
                "after" => {
                    let target = tokens.get(i + 1)?;
                    if target == "reversed" || target == "after" || after.contains(target) {
                        return None;
                    }
                    after.push(target.clone());
                    i += 2;
                }
                _ => return None,
            }
        }

        Some(DocumentItem::RemapGroup {
            name,
            reversed,
            after,
            comment,
        })
    }

    fn parse_assert_shape(tokens: &[String], comment: Option<String>) -> Option<DocumentItem> {
        if tokens.is_empty() {
            return None;
        }
        let text = tokens[0].clone();

        let first_colon = tokens.iter().position(|t| t == ":")?;

        let mut features = Vec::new();
        let mut language = None;
        // `for SLICE...` runs to the first `:`, so everything after it in the
        // pre-colon region is a slice name rather than another flag.
        let head = &tokens[1..first_colon];
        let for_pos = head.iter().position(|t| t == "for");
        let slices: Vec<String> = match for_pos {
            Some(i) => head[i + 1..].to_vec(),
            None => Vec::new(),
        };
        for tok in &head[..for_pos.unwrap_or(head.len())] {
            if let Some(tag) = tok.strip_prefix('+') {
                features.push(ShapeFeatureFlag {
                    tag: tag.to_string(),
                    enable: true,
                });
            } else if let Some(tag) = tok.strip_prefix('-') {
                features.push(ShapeFeatureFlag {
                    tag: tag.to_string(),
                    enable: false,
                });
            } else if let Some(tag) = tok.strip_prefix('@')
                && !tag.is_empty()
                && language.is_none()
            {
                language = Some(tag.to_string());
            }
        }
        // `for` with nothing after it states a constraint it does not carry.
        if for_pos.is_some() && slices.is_empty() {
            return None;
        }

        let glyph_tokens = &tokens[first_colon + 1..];
        let mut expected = Vec::new();
        let mut segments: Vec<&[String]> = Vec::new();

        let mut start = 0;
        for (i, tok) in glyph_tokens.iter().enumerate() {
            if tok == ":" && i > start {
                segments.push(&glyph_tokens[start..i]);
                start = i + 1;
            }
        }
        if start < glyph_tokens.len() {
            segments.push(&glyph_tokens[start..]);
        }

        for seg in segments {
            if seg.is_empty() {
                continue;
            }
            let name = seg[0].clone();
            let mut advance = None;
            let mut offset = None;
            let mut i = 1;
            while i < seg.len() {
                match seg[i].as_str() {
                    "advance" if i + 1 < seg.len() => {
                        advance = seg[i + 1].parse().ok();
                        i += 2;
                    }
                    "offset" if i + 2 < seg.len() => {
                        if let (Ok(x), Ok(y)) = (seg[i + 1].parse(), seg[i + 2].parse()) {
                            offset = Some((x, y));
                        }
                        i += 3;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
            expected.push(ExpectedGlyph {
                name,
                advance,
                offset,
            });
        }

        if expected.is_empty() {
            return None;
        }

        Some(DocumentItem::AssertShape {
            slices,
            text,
            features,
            language,
            expected,
            comment,
        })
    }

    /// The lines a [`Sample`](DocumentItem::Sample) is written as: its header
    /// and one `||` line per line of its text. `None` for every other item.
    ///
    /// Separate from [`serialize_line`](Self::serialize_line) because it is the
    /// one item that is more than a line, and a caller that means to write one
    /// line has to say which.
    ///
    /// The text is written back with exactly one space after each marker, which
    /// is what makes the round trip stable: the model's text is already
    /// dedented (see [`crate::document_io::dedent_continuations`]), so at least
    /// one of its non-blank lines starts at column 0 and the prefix a re-parse
    /// finds is that one space and nothing more. A blank line is written as a
    /// bare `||` rather than a marker and a trailing space.
    #[cfg(any(feature = "editor", test))]
    pub fn sample_lines(&self) -> Option<Vec<String>> {
        use crate::document_io::{CONTINUATION, quote_token};
        let DocumentItem::Sample {
            label,
            sublabel,
            mode,
            text,
            comment,
        } = self
        else {
            return None;
        };
        let mut header = format!("sample {}", quote_token(label));
        if let Some(sublabel) = sublabel {
            header.push(' ');
            header.push_str(&quote_token(sublabel));
        }
        if !mode.is_empty() {
            let qmode: Vec<String> = mode.iter().map(|m| quote_token(m)).collect();
            header.push_str(&format!(" : {}", qmode.join(" ")));
        }
        header.push_str(&serialize_comment_suffix(comment));
        let mut lines = vec![header];
        lines.extend(text.iter().map(|line| {
            if line.is_empty() {
                CONTINUATION.to_string()
            } else {
                format!("{CONTINUATION} {line}")
            }
        }));
        Some(lines)
    }

    #[cfg(any(feature = "editor", test))]
    pub fn serialize_line(&self) -> Option<String> {
        use crate::document_io::quote_token;
        match self {
            DocumentItem::Exists { pattern, comment } => Some(format!(
                "exists {}{}",
                quote_token(pattern),
                serialize_comment_suffix(comment),
            )),
            DocumentItem::NameParts {
                slices,
                name,
                values,
                comment,
            } => {
                let qvals: Vec<String> = values.iter().map(|v| quote_token(v)).collect();
                Some(format!(
                    "name-parts {}{} = {}{}",
                    serialize_slice_prefix(slices),
                    quote_token(name),
                    qvals.join(" "),
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::Remap {
                feature,
                lookbehind,
                source,
                target,
                lookahead,
                comment,
            } => {
                let mut parts = vec![format!("remap {} :", quote_token(feature))];
                if !lookbehind.is_empty() {
                    let lb: Vec<String> = lookbehind.iter().map(|s| quote_token(s)).collect();
                    parts.push(format!("{} :", lb.join(" ")));
                }
                let qs: Vec<String> = source.iter().map(|s| quote_token(s)).collect();
                let qt: Vec<String> = target.iter().map(|s| quote_token(s)).collect();
                parts.push(format!("{} -> {}", qs.join(" "), qt.join(" ")));
                if !lookahead.is_empty() {
                    let la: Vec<String> = lookahead.iter().map(|s| quote_token(s)).collect();
                    parts.push(format!(": {}", la.join(" ")));
                }
                Some(format!(
                    "{}{}",
                    parts.join(" "),
                    serialize_comment_suffix(comment)
                ))
            }
            DocumentItem::RemapGroup {
                name,
                reversed,
                after,
                comment,
            } => {
                let mut line = format!("remap group {}", quote_token(name));
                if *reversed {
                    line.push_str(" reversed");
                }
                for target in after {
                    line.push_str(&format!(" after {}", quote_token(target)));
                }
                Some(format!("{}{}", line, serialize_comment_suffix(comment)))
            }
            DocumentItem::Feature {
                slices,
                name,
                scripts,
                remap_group,
                comment,
            } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                Some(format!(
                    "feature {}{} for {} : {}{}",
                    serialize_slice_prefix(slices),
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(remap_group),
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::FeatureAnchor {
                slices,
                name,
                scripts,
                anchor,
                align,
                comment,
            } => {
                let qscripts: Vec<String> = scripts.iter().map(|s| quote_token(s)).collect();
                let align = match align.to_token() {
                    Some(token) => format!(" align {token}"),
                    None => String::new(),
                };
                Some(format!(
                    "feature {}{} for {} : anchor {}{}{}",
                    serialize_slice_prefix(slices),
                    quote_token(name),
                    qscripts.join(" "),
                    quote_token(anchor),
                    align,
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::PropBlock {
                name,
                start,
                end,
                comment,
            } => Some(format!(
                "prop block {} = {}{}",
                quote_token(name),
                crate::ucd::format_block_range(*start, *end),
                serialize_comment_suffix(comment),
            )),
            DocumentItem::PropChar {
                char_repr,
                name,
                values,
                comment,
            } => {
                let mut line = format!("prop {}", quote_token(char_repr));
                if let Some(name) = name {
                    line.push_str(&format!(" = {}", quote_token(name)));
                }
                // Written in the order the brace group shows them, whatever
                // order the source stated them in.
                if let Some(gc) = &values.gc {
                    line.push_str(&format!(" gc {}", quote_token(gc)));
                }
                if let Some(ccc) = values.ccc {
                    line.push_str(&format!(" ccc {ccc}"));
                }
                if let Some(eaw) = &values.eaw {
                    line.push_str(&format!(" eaw {}", quote_token(eaw)));
                }
                Some(format!("{line}{}", serialize_comment_suffix(comment)))
            }
            DocumentItem::Color {
                name,
                value,
                visibility,
                comment,
            } => {
                let vis = match visibility {
                    Some(LayerVisibility::ColorOnly) => " coloronly",
                    Some(LayerVisibility::MonoOnly) => " monoonly",
                    _ => "",
                };
                Some(format!(
                    "color {} = {}{}{}",
                    quote_token(name),
                    quote_token(value),
                    vis,
                    serialize_comment_suffix(comment),
                ))
            }
            DocumentItem::Face {
                id,
                slices,
                comment,
            } => {
                let mut line = format!("face {}", quote_token(id));
                if !slices.is_empty() {
                    let q: Vec<String> = slices.iter().map(|s| quote_token(s)).collect();
                    line.push_str(&format!(" : {}", q.join(" ")));
                }
                Some(format!("{line}{}", serialize_comment_suffix(comment)))
            }
            DocumentItem::Slice {
                id,
                inherits,
                comment,
            } => {
                let mut line = format!("slice {}", quote_token(id));
                if !inherits.is_empty() {
                    let q: Vec<String> = inherits.iter().map(|s| quote_token(s)).collect();
                    line.push_str(&format!(" = {}", q.join(" ")));
                }
                Some(format!("{line}{}", serialize_comment_suffix(comment)))
            }
            DocumentItem::AssertShape {
                slices,
                text,
                features,
                language,
                expected,
                comment,
            } => {
                let mut parts = vec!["assert".to_string(), "shape".to_string(), quote_token(text)];
                if let Some(lang) = language {
                    parts.push(format!("@{lang}"));
                }
                for f in features {
                    let prefix = if f.enable { "+" } else { "-" };
                    parts.push(format!("{prefix}{}", f.tag));
                }
                if !slices.is_empty() {
                    parts.push("for".to_string());
                    parts.extend(slices.iter().map(|s| quote_token(s)));
                }
                for (i, g) in expected.iter().enumerate() {
                    parts.push(":".to_string());
                    parts.push(quote_token(&g.name));
                    if let Some(adv) = g.advance {
                        parts.push("advance".to_string());
                        parts.push(adv.to_string());
                    }
                    if let Some((x, y)) = g.offset {
                        parts.push("offset".to_string());
                        parts.push(x.to_string());
                        parts.push(y.to_string());
                    }
                    let _ = i;
                }
                Some(format!(
                    "{}{}",
                    parts.join(" "),
                    serialize_comment_suffix(comment)
                ))
            }
            DocumentItem::AssertSame { names, comment } => {
                let qnames: Vec<String> = names.iter().map(|n| quote_token(n)).collect();
                Some(format!(
                    "assert same {}{}",
                    qnames.join(" "),
                    serialize_comment_suffix(comment)
                ))
            }
            DocumentItem::AssertDistinct { names, comment } => {
                let qnames: Vec<String> = names.iter().map(|n| quote_token(n)).collect();
                Some(format!(
                    "assert distinct {}{}",
                    qnames.join(" "),
                    serialize_comment_suffix(comment)
                ))
            }
            _ => None,
        }
    }
}
