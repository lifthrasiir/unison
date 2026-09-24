//! Lookups too large for one subtable's 16-bit offsets.
//!
//! write-fonts' packer promotes a lookup to an extension on its own, but splits
//! the subtables of only two GPOS types; a GSUB subtable past 64 KiB fails the
//! whole build. These build such lookups on purpose and read them back, checking
//! both that the table packs and that no rule was lost in the split.

use super::*;
use read_fonts::FontRead;
use read_fonts::tables::gsub::SubstitutionSubtables;

/// `count` distinct glyph names, `prefix0` … — each its own glyph id.
fn names(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|i| format!("{prefix}{i}")).collect()
}

fn assign_gids<'a>(
    name_to_gid: &mut HashMap<String, GlyphId16>,
    names: impl IntoIterator<Item = &'a String>,
) {
    for name in names {
        let next = GlyphId16::new(name_to_gid.len() as u16 + 1);
        name_to_gid.entry(name.clone()).or_insert(next);
    }
}

/// One group of `remaps`, attached to `liga` so it is reachable.
fn gsub_data_of(group: &str, remaps: Vec<ExpandedRemap>) -> GsubData {
    let mut groups = crate::document::RemapGroupOrder::default();
    groups.order.push(group.to_string());
    GsubData {
        remap_sets: BTreeMap::from([(group.to_string(), remaps)]),
        groups,
        features: vec![(
            "liga".to_string(),
            vec!["DFLT".to_string()],
            vec![group.to_string()],
        )],
        anchor_features: Vec::new(),
        uvs_pairs: Vec::new(),
        uvs_selectors: Vec::new(),
    }
}

fn remap(source: Vec<Vec<String>>, target: Vec<Vec<String>>) -> ExpandedRemap {
    ExpandedRemap {
        origin: None,
        lookbehind: Vec::new(),
        source,
        target,
        lookahead: Vec::new(),
    }
}

/// The built table, serialized and read back.
fn pack(gsub: &Gsub) -> Vec<u8> {
    write_fonts::dump_table(gsub).expect("GSUB should pack")
}

/// Every (first glyph, rest, ligature glyph) of lookup `index`, in subtable
/// order, and how many subtables carried them.
fn read_ligatures(bytes: &[u8], index: usize) -> (Vec<(u16, Vec<u16>, u16)>, usize) {
    let gsub = read_fonts::tables::gsub::Gsub::read(bytes.into()).unwrap();
    let lookup = gsub.lookup_list().unwrap().lookups().get(index).unwrap();
    let SubstitutionSubtables::Ligature(subtables) = lookup.subtables().unwrap() else {
        panic!("lookup {index} should be a ligature lookup");
    };
    let mut out = Vec::new();
    for subtable in subtables.iter() {
        let subtable = subtable.unwrap();
        let coverage = subtable.coverage().unwrap();
        for (first, set) in coverage.iter().zip(subtable.ligature_sets().iter()) {
            for lig in set.unwrap().ligatures().iter() {
                let lig = lig.unwrap();
                let rest = lig
                    .component_glyph_ids()
                    .iter()
                    .map(|g| g.get().to_u16())
                    .collect();
                out.push((first.to_u16(), rest, lig.ligature_glyph().to_u16()));
            }
        }
    }
    (out, subtables.len())
}

/// ~140 KiB of variation sequences: the shape of a CJK face with a few
/// thousand ideographs, each with its regional and IVD selectors.
#[test]
fn a_uvs_fallback_past_one_subtable_packs_and_keeps_every_pair() {
    const BASES: u32 = 2000;
    let selectors: Vec<u32> = (0xE0100..0xE0108).collect();

    let mut name_to_gid = HashMap::default();
    let mut cp_to_gid = HashMap::default();
    let mut gsub_data = gsub_data_of("unused", Vec::new());
    gsub_data.features.clear();
    let sel_names: Vec<String> = selectors.iter().map(|&s| vs_glyph_name(s)).collect();
    assign_gids(&mut name_to_gid, &sel_names);
    for base in 0x4E00..0x4E00 + BASES {
        let base_name = format!("base{base:X}");
        assign_gids(&mut name_to_gid, [&base_name]);
        cp_to_gid.insert(base, name_to_gid[&base_name]);
        for &selector in &selectors {
            let glyph = format!("var{base:X}-{selector:X}");
            assign_gids(&mut name_to_gid, [&glyph]);
            gsub_data.uvs_pairs.push(UvsPair {
                base,
                selector,
                glyph,
            });
        }
    }

    let gsub = build_gsub(&gsub_data, &name_to_gid, &cp_to_gid).expect("GSUB");
    let bytes = pack(&gsub);
    let (ligatures, subtables) = read_ligatures(&bytes, 0);

    let mut expected: Vec<(u16, Vec<u16>, u16)> = gsub_data
        .uvs_pairs
        .iter()
        .map(|p| {
            (
                cp_to_gid[&p.base].to_u16(),
                vec![name_to_gid[&vs_glyph_name(p.selector)].to_u16()],
                name_to_gid[&p.glyph].to_u16(),
            )
        })
        .collect();
    expected.sort();
    let mut got = ligatures;
    got.sort();
    assert_eq!(got, expected);
    // ~140 KiB needs three; splitting much finer than the offsets require
    // only costs subtable headers and lookup time.
    assert!((3..=4).contains(&subtables), "{subtables} subtables");
}

/// The same limit reached through a `remap` group of ligatures, whose order
/// within one first glyph (longest first) has to survive the split.
#[test]
fn a_ligature_group_past_one_subtable_packs_and_keeps_its_order() {
    let firsts = names("f", 2000);
    let seconds = names("s", 8);
    let thirds = names("t", 2);
    let mut targets = Vec::new();
    let mut sources = Vec::new();
    for f in &firsts {
        for s in &seconds {
            sources.push(vec![f.clone(), s.clone()]);
            targets.push(vec![format!("{f}-{s}")]);
        }
        sources.push(vec![f.clone(), seconds[0].clone(), thirds[0].clone()]);
        targets.push(vec![format!("{f}-long")]);
    }
    let mut name_to_gid = HashMap::default();
    assign_gids(
        &mut name_to_gid,
        firsts.iter().chain(&seconds).chain(&thirds),
    );
    assign_gids(&mut name_to_gid, targets.iter().flatten());

    let gsub_data = gsub_data_of("ligs", vec![remap(sources, targets)]);
    let gsub = build_gsub(&gsub_data, &name_to_gid, &HashMap::default()).expect("GSUB");
    let bytes = pack(&gsub);
    let (ligatures, subtables) = read_ligatures(&bytes, 0);

    assert_eq!(ligatures.len(), firsts.len() * (seconds.len() + 1));
    assert!(subtables >= 2, "{subtables} subtables");
    // Each first glyph's set leads with its three-glyph ligature.
    let long = ligatures.iter().filter(|(_, rest, _)| rest.len() == 2);
    for (first, _, _) in long {
        let set_head = ligatures.iter().find(|(f, _, _)| f == first).unwrap();
        assert_eq!(
            set_head.1.len(),
            2,
            "set of glyph {first} starts with a short ligature"
        );
    }
}

/// And through a group of one-to-many substitutions.
#[test]
fn a_multiple_group_past_one_subtable_packs_and_keeps_every_sequence() {
    let sources = names("m", 2000);
    let parts = names("p", 20);
    let mut name_to_gid = HashMap::default();
    assign_gids(&mut name_to_gid, sources.iter().chain(&parts));

    let rules = remap(
        sources.iter().map(|s| vec![s.clone()]).collect(),
        // Led by the source itself: identical sequences would be shared by
        // the packer and never reach the limit.
        sources
            .iter()
            .map(|s| std::iter::once(s).chain(&parts).cloned().collect())
            .collect(),
    );
    let gsub_data = gsub_data_of("multi", vec![rules]);
    let gsub = build_gsub(&gsub_data, &name_to_gid, &HashMap::default()).expect("GSUB");
    let bytes = pack(&gsub);

    let read = read_fonts::tables::gsub::Gsub::read(bytes.as_slice().into()).unwrap();
    let lookup = read.lookup_list().unwrap().lookups().get(0).unwrap();
    let SubstitutionSubtables::Multiple(subtables) = lookup.subtables().unwrap() else {
        panic!("lookup 0 should be a multiple substitution");
    };
    assert!(subtables.len() >= 2, "{} subtables", subtables.len());
    let mut covered = 0;
    for subtable in subtables.iter() {
        let subtable = subtable.unwrap();
        for seq in subtable.sequences().iter() {
            assert_eq!(seq.unwrap().substitute_glyph_ids().len(), parts.len() + 1);
            covered += 1;
        }
    }
    assert_eq!(covered, sources.len());
}
