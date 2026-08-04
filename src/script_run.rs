//! Script itemization (UAX #24).
//!
//! HarfBuzz deliberately does not itemize: `hb_buffer_guess_segment_properties()`
//! picks a single script for the *whole* buffer from its first strong character.
//! Feeding it mixed-script text therefore runs the wrong complex shaper over the
//! rest of the text — e.g. `ひ 각〯` gets shaped as Hiragana, so the Hangul
//! shaper never runs and U+AC01 ends up decomposed into jamo with the tone mark
//! left trailing. Callers must split the text into single-script runs first,
//! shape each run on its own, and concatenate the results.
//!
//! The resolution follows what browsers do: accumulate the intersection of the
//! Script_Extensions of the run so far, and start a new run when a character
//! does not intersect it. Common (`Zyyy`) and Inherited (`Zinh`) intersect
//! everything, so spaces and shared punctuation stay attached to the preceding
//! run instead of forming runs of their own. (Chrome and Firefox additionally
//! keep a paired-bracket stack so that `(` and `)` around a run agree; that is
//! more machinery than this preview needs.)
//!
//! Private-use and unassigned characters have *empty* Script_Extensions, so
//! intersection alone would give every one of them a run of its own and no GSUB
//! rule spanning two of them could ever fire. A PUA-encoded script is still
//! a script; such characters are tracked as one `Unknown` run that groups with
//! itself and with Common / Inherited, but never merges with a real script.

use std::ops::Range;

use unicode_script::{ScriptExtension, UnicodeScript};

/// One maximal single-script stretch of the input text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptRun {
    /// Byte range within the original text.
    pub bytes: Range<usize>,
    /// Index of the run's first character within the original text, for
    /// rebasing per-run cluster indices back to the whole text.
    pub char_start: usize,
}

/// Split `text` into runs that may each be shaped as a single script.
///
/// Returns an empty vector for empty input; otherwise the runs tile the whole
/// text in order, with no gaps.
pub fn split_script_runs(text: &str) -> Vec<ScriptRun> {
    let mut runs = Vec::new();
    if text.is_empty() {
        return runs;
    }

    /// What the run built so far is compatible with.
    enum RunScript {
        /// Only Common/Inherited so far; adopts whatever comes next.
        Any,
        /// Intersection of the Script_Extensions seen so far.
        Known(ScriptExtension),
        /// Private-use or unassigned characters, which carry no scripts.
        Unknown,
    }

    let mut current = RunScript::Any;
    let mut start = 0;
    let mut char_start = 0;

    for (char_idx, (byte_idx, ch)) in text.char_indices().enumerate() {
        let ext = ch.script_extension();
        let next = match (&current, ext.is_empty()) {
            // Private use / unassigned: joins an `Any` or `Unknown` run.
            (RunScript::Any | RunScript::Unknown, true) => Some(RunScript::Unknown),
            // Common and Inherited stay inside an unknown run, like everywhere else.
            (RunScript::Unknown, false) if ext.is_common() || ext.is_inherited() => {
                Some(RunScript::Unknown)
            }
            (RunScript::Any, false) => Some(RunScript::Known(ext)),
            (RunScript::Known(cur), false) => {
                let merged = cur.intersection(ext);
                (!merged.is_empty()).then_some(RunScript::Known(merged))
            }
            // Known ↔ Unknown never merge in either direction.
            (RunScript::Known(_), true) | (RunScript::Unknown, false) => None,
        };
        current = match next {
            Some(next) => next,
            None => {
                runs.push(ScriptRun {
                    bytes: start..byte_idx,
                    char_start,
                });
                start = byte_idx;
                char_start = char_idx;
                if ext.is_empty() {
                    RunScript::Unknown
                } else {
                    RunScript::Known(ext)
                }
            }
        };
    }
    runs.push(ScriptRun {
        bytes: start..text.len(),
        char_start,
    });

    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs_of(text: &str) -> Vec<&str> {
        split_script_runs(text)
            .into_iter()
            .map(|r| &text[r.bytes])
            .collect()
    }

    #[test]
    fn empty_text_has_no_runs() {
        assert!(split_script_runs("").is_empty());
    }

    #[test]
    fn single_script_stays_one_run() {
        assert_eq!(runs_of("가나다"), vec!["가나다"]);
        assert_eq!(runs_of("hello"), vec!["hello"]);
    }

    #[test]
    fn different_scripts_are_split() {
        assert_eq!(runs_of("ひ가"), vec!["ひ", "가"]);
        assert_eq!(runs_of("abc가나ひら"), vec!["abc", "가나", "ひら"]);
    }

    #[test]
    fn common_attaches_to_preceding_run() {
        // The space must not become a run of its own, and must not carry the
        // Hiragana run's script over into the Hangul run.
        assert_eq!(runs_of("ひ 각"), vec!["ひ ", "각"]);
        assert_eq!(runs_of("가 나"), vec!["가 나"]);
    }

    #[test]
    fn leading_common_adopts_following_script() {
        assert_eq!(runs_of("  가나"), vec!["  가나"]);
        assert_eq!(runs_of("123가나ひ"), vec!["123가나", "ひ"]);
    }

    #[test]
    fn hangul_tone_mark_stays_with_syllable() {
        // U+302F has Script_Extensions = { Hangul }.
        assert_eq!(runs_of("ひ 각\u{302F}"), vec!["ひ ", "각\u{302F}"]);
    }

    #[test]
    fn script_extensions_keep_shared_punctuation_together() {
        // U+3001 IDEOGRAPHIC COMMA has scx = { Bopo, Hang, Hani, Hira, Kana, ... },
        // so it stays inside the kana run rather than splitting it.
        assert_eq!(runs_of("あ、い"), vec!["あ、い"]);
    }

    #[test]
    fn combining_marks_stay_with_their_base() {
        // U+0308 is Inherited.
        assert_eq!(runs_of("i\u{0308}가"), vec!["i\u{0308}", "가"]);
    }

    #[test]
    fn private_use_stays_one_run() {
        // A PUA-encoded script (here sitelen pona in Plane 15) has no assigned
        // Script_Extensions, but its characters still have to reach the shaper
        // as one buffer or no GSUB rule between them can ever fire.
        assert_eq!(
            runs_of("\u{F194D}\u{F1997}\u{F1998}"),
            vec!["\u{F194D}\u{F1997}\u{F1998}"]
        );
        // Private use must still not merge with a real script.
        assert_eq!(runs_of("\u{F194D}가"), vec!["\u{F194D}", "가"]);
    }

    #[test]
    fn runs_tile_the_whole_text_without_gaps() {
        let text = "ひ 각\u{302F}abc가\u{0308}、あ";
        let runs = split_script_runs(text);
        assert_eq!(runs.first().unwrap().bytes.start, 0);
        assert_eq!(runs.last().unwrap().bytes.end, text.len());
        for pair in runs.windows(2) {
            assert_eq!(pair[0].bytes.end, pair[1].bytes.start);
        }
        for run in &runs {
            assert_eq!(run.char_start, text[..run.bytes.start].chars().count());
        }
    }
}
