//! Fixtures are written inline rather than read out of `font/`: the shapes
//! being tested are a handful of rules each, and the real files change for
//! font-design reasons.

use super::*;

/// A `remap` line under construction, so a fixture reads like the source line it
/// stands for.
#[derive(Default)]
struct Line {
    lookbehind: Vec<Vec<String>>,
    source: Vec<Vec<String>>,
    target: Vec<Vec<String>>,
    lookahead: Vec<Vec<String>>,
}

impl Line {
    /// `source` and `target` are space-separated glyph sequences.
    fn new(source: &str, target: &str) -> Self {
        Line::default().alt(source, target)
    }

    /// Another expansion of the same line, sharing its context.
    fn alt(mut self, source: &str, target: &str) -> Self {
        self.source.push(seq(source));
        self.target.push(seq(target));
        self
    }

    /// One lookbehind position, given as its alternatives. Called in reading
    /// order, so the last call is the position next to the input.
    fn before(mut self, alternatives: &str) -> Self {
        self.lookbehind.push(seq(alternatives));
        self
    }

    /// One lookahead position, given as its alternatives.
    fn after(mut self, alternatives: &str) -> Self {
        self.lookahead.push(seq(alternatives));
        self
    }

    fn as_line(&self) -> RemapLine<'_> {
        RemapLine {
            lookbehind: &self.lookbehind,
            source: &self.source,
            target: &self.target,
            lookahead: &self.lookahead,
        }
    }
}

fn seq(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

fn cmap(pairs: &[(u32, &str)]) -> Vec<(u32, String)> {
    pairs.iter().map(|(cp, n)| (*cp, n.to_string())).collect()
}

fn groups(source: &[Vec<Line>]) -> Vec<Vec<RemapLine<'_>>> {
    source
        .iter()
        .map(|group| group.iter().map(Line::as_line).collect())
        .collect()
}

const NO_UVS: &[(u32, u32, String)] = &[];

fn solved<'a>(
    cascade: &Cascade<'a>,
    targets: &[&str],
) -> std::collections::BTreeMap<&'a str, Vec<u32>> {
    cascade.solve(&targets.iter().copied().collect())
}

// A: 0x41, B: 0x42 — short enough to read in an assertion.
const A: u32 = 0x41;
const B: u32 = 0x42;
const C: u32 = 0x43;

#[test]
fn a_mapped_glyph_is_typed_as_itself() {
    let cm = cmap(&[(A, "a")]);
    let no_groups: Vec<Vec<RemapLine>> = Vec::new();
    let cascade = Cascade::new(&cm, NO_UVS, &no_groups);

    assert_eq!(solved(&cascade, &["a"]).get("a"), Some(&vec![A]));
}

#[test]
fn a_ligature_is_typed_as_its_inputs() {
    let cm = cmap(&[(A, "a"), (B, "b")]);
    let src = vec![vec![Line::new("a b", "ab")]];
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    assert_eq!(solved(&cascade, &["ab"]).get("ab"), Some(&vec![A, B]));
}

#[test]
fn an_unreachable_glyph_gets_no_sequence() {
    let cm = cmap(&[(A, "a")]);
    // Nothing writes `ghost`, so nothing can write `phantom` either.
    let src = vec![vec![Line::new("ghost", "phantom")]];
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    assert!(solved(&cascade, &["phantom"]).is_empty());
}

/// The Hangul shape: one jamo becomes a different variant depending on what
/// follows it, and the sequence has to carry that context.
fn jamo_source() -> Vec<Vec<Line>> {
    vec![vec![
        // The longer rule comes first, or the shorter one eats its prefix.
        Line::new("init", "init-with-final").after("med").after("fin"),
        Line::new("init", "init-bare").after("med"),
    ]]
}

const INIT: u32 = 0x1100;
const MED: u32 = 0x1161;
const FIN: u32 = 0x11A8;

fn jamo_cmap() -> Vec<(u32, String)> {
    cmap(&[(INIT, "init"), (MED, "med"), (FIN, "fin")])
}

#[test]
fn a_rule_is_typed_with_the_lookahead_it_needs() {
    let cm = jamo_cmap();
    let src = jamo_source();
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    let out = solved(&cascade, &["init-bare", "init-with-final"]);
    // The context is part of what has to be typed, and the two variants differ
    // by exactly the trailing jamo that selects them.
    assert_eq!(out.get("init-bare"), Some(&vec![INIT, MED]));
    assert_eq!(out.get("init-with-final"), Some(&vec![INIT, MED, FIN]));
}

#[test]
fn the_shorter_rule_placed_first_shadows_the_longer_one() {
    let cm = jamo_cmap();
    let mut src = jamo_source();
    src[0].reverse();
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    let out = solved(&cascade, &["init-bare", "init-with-final"]);
    // `init-bare` matches wherever `init-with-final` does, so put first it
    // takes every position and the longer rule never fires. Nothing is reported
    // for a glyph the font cannot actually be made to show.
    assert_eq!(out.get("init-bare"), Some(&vec![INIT, MED]));
    assert_eq!(out.get("init-with-final"), None);
}

#[test]
fn a_group_does_not_feed_its_own_rules_at_one_position() {
    let cm = cmap(&[(A, "a")]);
    let one = vec![vec![Line::new("a", "b"), Line::new("b", "c")]];
    let gs = groups(&one);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);
    // One lookup is one pass: the second rule never sees what the first wrote
    // at the position the pass has already left.
    assert_eq!(solved(&cascade, &["c"]).get("c"), None);

    let two = vec![vec![Line::new("a", "b")], vec![Line::new("b", "c")]];
    let gs = groups(&two);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);
    assert_eq!(solved(&cascade, &["c"]).get("c"), Some(&vec![A]));
}

/// `font/flags.unf`'s shape, with two indicators instead of twenty-six: one
/// group decides left from right, a second pairs them.
fn flag_source() -> Vec<Vec<Line>> {
    vec![
        vec![
            Line::new("ri-a", "ri-a-right")
                .alt("ri-b", "ri-b-right")
                .before("ri-a-left ri-b-left"),
            Line::new("ri-a", "ri-a-left")
                .alt("ri-b", "ri-b-left")
                .after("ri-a ri-b"),
        ],
        vec![Line::new("ri-a-left ri-b-right", "flag-ab").alt("ri-a-left ri-a-right", "flag-aa")],
    ]
}

const RI_A: u32 = 0x1F1E6;
const RI_B: u32 = 0x1F1E7;

fn flag_cmap() -> Vec<(u32, String)> {
    cmap(&[(RI_A, "ri-a"), (RI_B, "ri-b")])
}

#[test]
fn regional_indicators_pair_into_a_flag() {
    let cm = flag_cmap();
    let src = flag_source();
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    let out = solved(&cascade, &["flag-ab", "flag-aa"]);
    assert_eq!(out.get("flag-ab"), Some(&vec![RI_A, RI_B]));
    assert_eq!(out.get("flag-aa"), Some(&vec![RI_A, RI_A]));
}

#[test]
fn the_per_glyph_sweep_alone_would_answer_the_flag_wrongly() {
    let cm = flag_cmap();
    let src = flag_source();
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    // This is why the answer is checked instead of derived. `ri-a-left` is
    // written with a following indicator as its lookahead and `ri-b-right` is
    // written *from* one, and those are the same position in the string — so
    // adding the two derivations up writes it twice.
    let candidate = cascade.sweep();
    let guess = candidate.get("flag-ab").expect("the sweep does reach it");
    assert!(guess.len() > 2, "the sweep is expected to overcount: {guess:?}");

    // And it is not merely long: five indicators pair up as two other flags and
    // a leftover, so the sequence renders no `flag-ab` at all.
    let run = cascade.shape(guess).expect("every code point is mapped");
    assert!(!run.contains(&"flag-ab"), "{run:?}");
}

#[test]
fn a_self_referential_lookbehind_terminates() {
    // `tok-long`'s shape: the rule's own output is in its lookbehind coverage,
    // so the language is unbounded. The shortest derivation never repeats it.
    const START: u32 = 0x21;
    const X: u32 = 0x58;
    let cm = cmap(&[(START, "start"), (X, "x")]);
    let src = vec![vec![Line::new("x", "x cont").before("start cont")]];
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    assert_eq!(solved(&cascade, &["cont"]).get("cont"), Some(&vec![START, X]));
}

#[test]
fn a_variation_sequence_is_two_code_points() {
    const VS: u32 = 0xFE00;
    let cm = cmap(&[(A, "a")]);
    let uvs = vec![(A, VS, "a-alt".to_string())];
    let src = vec![vec![Line::new("a-alt", "a-fancy")]];
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, &uvs, &gs);

    assert_eq!(
        solved(&cascade, &["a-fancy"]).get("a-fancy"),
        Some(&vec![A, VS])
    );
}

#[test]
fn the_answer_does_not_depend_on_the_order_the_targets_are_asked_in() {
    let cm = flag_cmap();
    let src = flag_source();
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    let forward = solved(&cascade, &["flag-aa", "flag-ab"]);
    let backward = solved(&cascade, &["flag-ab", "flag-aa"]);
    assert_eq!(forward, backward);
}

/// Three indicators, every pair of the first two making a flag and the third
/// pairing with nothing.
fn crowded_flag_source() -> Vec<Vec<Line>> {
    vec![
        vec![
            Line::new("ri-a", "ri-a-right")
                .alt("ri-b", "ri-b-right")
                .alt("ri-c", "ri-c-right")
                .before("ri-a-left ri-b-left ri-c-left"),
            Line::new("ri-a", "ri-a-left")
                .alt("ri-b", "ri-b-left")
                .alt("ri-c", "ri-c-left")
                .after("ri-a ri-b ri-c"),
        ],
        vec![
            Line::new("ri-a-left ri-a-right", "flag-aa")
                .alt("ri-a-left ri-b-right", "flag-ab")
                .alt("ri-b-left ri-a-right", "flag-ba")
                .alt("ri-b-left ri-b-right", "flag-bb"),
        ],
    ]
}

#[test]
fn a_glyph_a_later_group_consumes_needs_more_than_its_own_candidate() {
    let cm = cmap(&[(A, "ri-a"), (B, "ri-b"), (C, "ri-c")]);
    let src = crowded_flag_source();
    let gs = groups(&src);
    let cascade = Cascade::new(&cm, NO_UVS, &gs);

    // The candidate names one code point, and every word over it is a flag the
    // second group swallows — so `ri-a-left` can never be seen by writing what
    // its own derivation mentions.
    let guess = cascade.sweep().get("ri-a-left").cloned().expect("reached");
    let named: BTreeSet<u32> = guess.iter().copied().collect();
    assert_eq!(named, BTreeSet::from([A]));

    // Widening to what the producing rules can spell finds the third indicator,
    // which pairs with nothing and so leaves `ri-a-left` standing.
    let answer = solved(&cascade, &["ri-a-left"])
        .get("ri-a-left")
        .cloned()
        .expect("the wider alphabet reaches it");
    assert!(
        answer.iter().any(|cp| !named.contains(cp)),
        "the answer has to name a code point the candidate does not: {answer:?}"
    );
    let run = cascade.shape(&answer).expect("every code point is mapped");
    assert!(run.contains(&"ri-a-left"), "{run:?}");
}
