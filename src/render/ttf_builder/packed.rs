//! `GSUB` packed once per distinct table, not once per build.
//!
//! Packing is write-fonts' repacker: a graph of every lookup, subtable,
//! coverage and class table, sorted so that every 16-bit offset reaches its
//! target. Past 64 KiB no first ordering does that — a coverage table shared by
//! subtables far apart overflows from one of them whichever way it is placed —
//! so it falls through to the HarfBuzz strategy: a shortest-distance sort,
//! extension promotion, space assignment, isolating and duplicating subgraphs,
//! and a sort after each. At twenty thousand objects that is half of
//! `font pair: tables`, for each flavor, and the part of it that grew fastest
//! with the font. Promoting every lookup to an extension up front does not
//! avoid it: the offsets that overflow are the subtables' own, below the
//! extension.
//!
//! Yet the table rarely changes. An edit to a drawing leaves every rule where
//! it was, and so do the bitmap and vector flavors of one build, and the faces
//! of a collection that share their rules. So the packed bytes are memoized
//! under the table itself, compared with `==`: no key to collide, and nothing
//! to keep in step with what the packer reads, because the key *is* what it
//! reads. The comparison is a walk over the same objects the packer would
//! sort, without the sorting.
//!
//! The memo holds the last few tables, which is one per flavor and face of
//! the editor's current build with room to spare. The lock is held only to
//! look entries up and to file one; the comparison and any packing run outside
//! it, so the flavors and faces built at once do not queue on each other.

use std::sync::{Arc, Mutex};

use write_fonts::tables::gsub::Gsub;

/// How many distinct tables the memo keeps, most recently used first.
const CAPACITY: usize = 4;

type Entry = Arc<(Gsub, Arc<[u8]>)>;

static MEMO: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// `gsub` packed, as [`write_fonts::dump_table`] packs it.
pub(super) fn packed_gsub(gsub: &Gsub) -> Result<Arc<[u8]>, write_fonts::error::Error> {
    let entries: Vec<Entry> = crate::parallel::lock_memo(&MEMO).clone();
    if let Some(hit) = entries.iter().find(|e| e.0 == *gsub) {
        let bytes = hit.1.clone();
        promote(hit);
        return Ok(bytes);
    }
    let bytes: Arc<[u8]> = write_fonts::dump_table(gsub)?.into();
    let entry = Arc::new((gsub.clone(), bytes.clone()));
    let mut memo = crate::parallel::lock_memo(&MEMO);
    memo.insert(0, entry);
    memo.truncate(CAPACITY);
    Ok(bytes)
}

/// Moves `hit` to the front, if another thread has not already dropped it.
fn promote(hit: &Entry) {
    let mut memo = crate::parallel::lock_memo(&MEMO);
    if let Some(at) = memo.iter().position(|e| Arc::ptr_eq(e, hit)) {
        let entry = memo.remove(at);
        memo.insert(0, entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use write_fonts::tables::gsub::{SingleSubst, SubstitutionLookup};
    use write_fonts::tables::layout::{
        CoverageTable, Feature, FeatureList, FeatureRecord, LangSys, Lookup, LookupFlag,
        LookupList, Script, ScriptList, ScriptRecord,
    };
    use write_fonts::types::{GlyphId16, Tag};

    /// A one-lookup table substituting `from` by `to`.
    fn single(from: u16, to: u16) -> Gsub {
        let coverage = CoverageTable::format_1(vec![GlyphId16::new(from)]);
        let subtable = SingleSubst::format_1(coverage, to as i16 - from as i16);
        let lookup = SubstitutionLookup::Single(Lookup::new(LookupFlag::empty(), vec![subtable]));
        let script = Script::new(Some(LangSys::new(vec![0])), Vec::new());
        Gsub::new(
            ScriptList::new(vec![ScriptRecord::new(Tag::new(b"DFLT"), script)]),
            FeatureList::new(vec![FeatureRecord::new(
                Tag::new(b"test"),
                Feature::new(None, vec![0]),
            )]),
            LookupList::new(vec![lookup]),
        )
    }

    #[test]
    fn a_memoized_table_packs_as_dump_table_does() {
        let gsub = single(1, 2);
        let fresh = write_fonts::dump_table(&gsub).unwrap();
        assert_eq!(&*packed_gsub(&gsub).unwrap(), fresh.as_slice());
        // The second call is served from the memo.
        assert_eq!(&*packed_gsub(&gsub).unwrap(), fresh.as_slice());
    }

    #[test]
    fn a_different_table_is_not_served_the_bytes_of_another() {
        let (a, b) = (single(3, 4), single(3, 5));
        let packed_a = packed_gsub(&a).unwrap();
        let packed_b = packed_gsub(&b).unwrap();
        assert_ne!(packed_a, packed_b);
        assert_eq!(&*packed_b, write_fonts::dump_table(&b).unwrap().as_slice());
        assert_eq!(
            &*packed_gsub(&a).unwrap(),
            write_fonts::dump_table(&a).unwrap().as_slice()
        );
    }
}
