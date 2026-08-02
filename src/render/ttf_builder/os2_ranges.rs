//! `OS/2.ulUnicodeRange` and `OS/2.ulCodePageRange`, computed from the cmap.
//!
//! Neither belongs in `meta`: they describe what the built font *ended up*
//! covering, so declaring them would only create a second source of truth that
//! drifts. Both used to be wrong — the unicode ranges were all zero ("covers
//! nothing", which Windows font fallback and font pickers believe), and the
//! code page ranges were hardcoded to Latin-1 in a font with Han, Hangul,
//! Cyrillic and Greek.
//!
//! [`UNICODE_RANGE_BITS`] is generated from the table in `fontTools`
//! (`O_S_2f_2.OS2_UNICODE_RANGES`) rather than transcribed by hand; 169 block
//! ranges over 123 bits is too much to get right from memory. Bits above 122
//! are reserved.

/// `(bit, first codepoint, last codepoint)` — one row per block a bit covers.
/// Several bits cover more than one block, so `bit` repeats.
#[rustfmt::skip]
pub(super) const UNICODE_RANGE_BITS: &[(u8, u32, u32)] = &[
    (0, 0x0000, 0x007F), // Basic Latin
    (1, 0x0080, 0x00FF), // Latin-1 Supplement
    (2, 0x0100, 0x017F), // Latin Extended-A
    (3, 0x0180, 0x024F), // Latin Extended-B
    (4, 0x0250, 0x02AF), // IPA Extensions
    (4, 0x1D00, 0x1D7F), // Phonetic Extensions
    (4, 0x1D80, 0x1DBF), // Phonetic Extensions Supplement
    (5, 0x02B0, 0x02FF), // Spacing Modifier Letters
    (5, 0xA700, 0xA71F), // Modifier Tone Letters
    (6, 0x0300, 0x036F), // Combining Diacritical Marks
    (6, 0x1DC0, 0x1DFF), // Combining Diacritical Marks Supplement
    (7, 0x0370, 0x03FF), // Greek and Coptic
    (8, 0x2C80, 0x2CFF), // Coptic
    (9, 0x0400, 0x04FF), // Cyrillic
    (9, 0x0500, 0x052F), // Cyrillic Supplement
    (9, 0x2DE0, 0x2DFF), // Cyrillic Extended-A
    (9, 0xA640, 0xA69F), // Cyrillic Extended-B
    (10, 0x0530, 0x058F), // Armenian
    (11, 0x0590, 0x05FF), // Hebrew
    (12, 0xA500, 0xA63F), // Vai
    (13, 0x0600, 0x06FF), // Arabic
    (13, 0x0750, 0x077F), // Arabic Supplement
    (14, 0x07C0, 0x07FF), // NKo
    (15, 0x0900, 0x097F), // Devanagari
    (16, 0x0980, 0x09FF), // Bengali
    (17, 0x0A00, 0x0A7F), // Gurmukhi
    (18, 0x0A80, 0x0AFF), // Gujarati
    (19, 0x0B00, 0x0B7F), // Oriya
    (20, 0x0B80, 0x0BFF), // Tamil
    (21, 0x0C00, 0x0C7F), // Telugu
    (22, 0x0C80, 0x0CFF), // Kannada
    (23, 0x0D00, 0x0D7F), // Malayalam
    (24, 0x0E00, 0x0E7F), // Thai
    (25, 0x0E80, 0x0EFF), // Lao
    (26, 0x10A0, 0x10FF), // Georgian
    (26, 0x2D00, 0x2D2F), // Georgian Supplement
    (27, 0x1B00, 0x1B7F), // Balinese
    (28, 0x1100, 0x11FF), // Hangul Jamo
    (29, 0x1E00, 0x1EFF), // Latin Extended Additional
    (29, 0x2C60, 0x2C7F), // Latin Extended-C
    (29, 0xA720, 0xA7FF), // Latin Extended-D
    (30, 0x1F00, 0x1FFF), // Greek Extended
    (31, 0x2000, 0x206F), // General Punctuation
    (31, 0x2E00, 0x2E7F), // Supplemental Punctuation
    (32, 0x2070, 0x209F), // Superscripts And Subscripts
    (33, 0x20A0, 0x20CF), // Currency Symbols
    (34, 0x20D0, 0x20FF), // Combining Diacritical Marks For Symbols
    (35, 0x2100, 0x214F), // Letterlike Symbols
    (36, 0x2150, 0x218F), // Number Forms
    (37, 0x2190, 0x21FF), // Arrows
    (37, 0x27F0, 0x27FF), // Supplemental Arrows-A
    (37, 0x2900, 0x297F), // Supplemental Arrows-B
    (37, 0x2B00, 0x2BFF), // Miscellaneous Symbols and Arrows
    (38, 0x2200, 0x22FF), // Mathematical Operators
    (38, 0x2A00, 0x2AFF), // Supplemental Mathematical Operators
    (38, 0x27C0, 0x27EF), // Miscellaneous Mathematical Symbols-A
    (38, 0x2980, 0x29FF), // Miscellaneous Mathematical Symbols-B
    (39, 0x2300, 0x23FF), // Miscellaneous Technical
    (40, 0x2400, 0x243F), // Control Pictures
    (41, 0x2440, 0x245F), // Optical Character Recognition
    (42, 0x2460, 0x24FF), // Enclosed Alphanumerics
    (43, 0x2500, 0x257F), // Box Drawing
    (44, 0x2580, 0x259F), // Block Elements
    (45, 0x25A0, 0x25FF), // Geometric Shapes
    (46, 0x2600, 0x26FF), // Miscellaneous Symbols
    (47, 0x2700, 0x27BF), // Dingbats
    (48, 0x3000, 0x303F), // CJK Symbols And Punctuation
    (49, 0x3040, 0x309F), // Hiragana
    (50, 0x30A0, 0x30FF), // Katakana
    (50, 0x31F0, 0x31FF), // Katakana Phonetic Extensions
    (51, 0x3100, 0x312F), // Bopomofo
    (51, 0x31A0, 0x31BF), // Bopomofo Extended
    (52, 0x3130, 0x318F), // Hangul Compatibility Jamo
    (53, 0xA840, 0xA87F), // Phags-pa
    (54, 0x3200, 0x32FF), // Enclosed CJK Letters And Months
    (55, 0x3300, 0x33FF), // CJK Compatibility
    (56, 0xAC00, 0xD7AF), // Hangul Syllables
    (57, 0xD800, 0xDFFF), // Non-Plane 0 *
    (58, 0x10900, 0x1091F), // Phoenician
    (59, 0x4E00, 0x9FFF), // CJK Unified Ideographs
    (59, 0x2E80, 0x2EFF), // CJK Radicals Supplement
    (59, 0x2F00, 0x2FDF), // Kangxi Radicals
    (59, 0x2FF0, 0x2FFF), // Ideographic Description Characters
    (59, 0x3400, 0x4DBF), // CJK Unified Ideographs Extension A
    (59, 0x20000, 0x2A6DF), // CJK Unified Ideographs Extension B
    (59, 0x3190, 0x319F), // Kanbun
    (60, 0xE000, 0xF8FF), // Private Use Area (plane 0)
    (61, 0x31C0, 0x31EF), // CJK Strokes
    (61, 0xF900, 0xFAFF), // CJK Compatibility Ideographs
    (61, 0x2F800, 0x2FA1F), // CJK Compatibility Ideographs Supplement
    (62, 0xFB00, 0xFB4F), // Alphabetic Presentation Forms
    (63, 0xFB50, 0xFDFF), // Arabic Presentation Forms-A
    (64, 0xFE20, 0xFE2F), // Combining Half Marks
    (65, 0xFE10, 0xFE1F), // Vertical Forms
    (65, 0xFE30, 0xFE4F), // CJK Compatibility Forms
    (66, 0xFE50, 0xFE6F), // Small Form Variants
    (67, 0xFE70, 0xFEFF), // Arabic Presentation Forms-B
    (68, 0xFF00, 0xFFEF), // Halfwidth And Fullwidth Forms
    (69, 0xFFF0, 0xFFFF), // Specials
    (70, 0x0F00, 0x0FFF), // Tibetan
    (71, 0x0700, 0x074F), // Syriac
    (72, 0x0780, 0x07BF), // Thaana
    (73, 0x0D80, 0x0DFF), // Sinhala
    (74, 0x1000, 0x109F), // Myanmar
    (75, 0x1200, 0x137F), // Ethiopic
    (75, 0x1380, 0x139F), // Ethiopic Supplement
    (75, 0x2D80, 0x2DDF), // Ethiopic Extended
    (76, 0x13A0, 0x13FF), // Cherokee
    (77, 0x1400, 0x167F), // Unified Canadian Aboriginal Syllabics
    (78, 0x1680, 0x169F), // Ogham
    (79, 0x16A0, 0x16FF), // Runic
    (80, 0x1780, 0x17FF), // Khmer
    (80, 0x19E0, 0x19FF), // Khmer Symbols
    (81, 0x1800, 0x18AF), // Mongolian
    (82, 0x2800, 0x28FF), // Braille Patterns
    (83, 0xA000, 0xA48F), // Yi Syllables
    (83, 0xA490, 0xA4CF), // Yi Radicals
    (84, 0x1700, 0x171F), // Tagalog
    (84, 0x1720, 0x173F), // Hanunoo
    (84, 0x1740, 0x175F), // Buhid
    (84, 0x1760, 0x177F), // Tagbanwa
    (85, 0x10300, 0x1032F), // Old Italic
    (86, 0x10330, 0x1034F), // Gothic
    (87, 0x10400, 0x1044F), // Deseret
    (88, 0x1D000, 0x1D0FF), // Byzantine Musical Symbols
    (88, 0x1D100, 0x1D1FF), // Musical Symbols
    (88, 0x1D200, 0x1D24F), // Ancient Greek Musical Notation
    (89, 0x1D400, 0x1D7FF), // Mathematical Alphanumeric Symbols
    (90, 0xF0000, 0xFFFFD), // Private Use (plane 15)
    (90, 0x100000, 0x10FFFD), // Private Use (plane 16)
    (91, 0xFE00, 0xFE0F), // Variation Selectors
    (91, 0xE0100, 0xE01EF), // Variation Selectors Supplement
    (92, 0xE0000, 0xE007F), // Tags
    (93, 0x1900, 0x194F), // Limbu
    (94, 0x1950, 0x197F), // Tai Le
    (95, 0x1980, 0x19DF), // New Tai Lue
    (96, 0x1A00, 0x1A1F), // Buginese
    (97, 0x2C00, 0x2C5F), // Glagolitic
    (98, 0x2D30, 0x2D7F), // Tifinagh
    (99, 0x4DC0, 0x4DFF), // Yijing Hexagram Symbols
    (100, 0xA800, 0xA82F), // Syloti Nagri
    (101, 0x10000, 0x1007F), // Linear B Syllabary
    (101, 0x10080, 0x100FF), // Linear B Ideograms
    (101, 0x10100, 0x1013F), // Aegean Numbers
    (102, 0x10140, 0x1018F), // Ancient Greek Numbers
    (103, 0x10380, 0x1039F), // Ugaritic
    (104, 0x103A0, 0x103DF), // Old Persian
    (105, 0x10450, 0x1047F), // Shavian
    (106, 0x10480, 0x104AF), // Osmanya
    (107, 0x10800, 0x1083F), // Cypriot Syllabary
    (108, 0x10A00, 0x10A5F), // Kharoshthi
    (109, 0x1D300, 0x1D35F), // Tai Xuan Jing Symbols
    (110, 0x12000, 0x123FF), // Cuneiform
    (110, 0x12400, 0x1247F), // Cuneiform Numbers and Punctuation
    (111, 0x1D360, 0x1D37F), // Counting Rod Numerals
    (112, 0x1B80, 0x1BBF), // Sundanese
    (113, 0x1C00, 0x1C4F), // Lepcha
    (114, 0x1C50, 0x1C7F), // Ol Chiki
    (115, 0xA880, 0xA8DF), // Saurashtra
    (116, 0xA900, 0xA92F), // Kayah Li
    (117, 0xA930, 0xA95F), // Rejang
    (118, 0xAA00, 0xAA5F), // Cham
    (119, 0x10190, 0x101CF), // Ancient Symbols
    (120, 0x101D0, 0x101FF), // Phaistos Disc
    (121, 0x102A0, 0x102DF), // Carian
    (121, 0x10280, 0x1029F), // Lycian
    (121, 0x10920, 0x1093F), // Lydian
    (122, 0x1F030, 0x1F09F), // Domino Tiles
    (122, 0x1F000, 0x1F02F), // Mahjong Tiles
];

/// The four `ulUnicodeRange` words, from every codepoint the cmap maps.
pub(super) fn unicode_ranges(codepoints: impl Iterator<Item = u32>) -> [u32; 4] {
    let mut out = [0u32; 4];
    for cp in codepoints {
        for &(bit, start, end) in UNICODE_RANGE_BITS {
            if (start..=end).contains(&cp) {
                out[(bit / 32) as usize] |= 1 << (bit % 32);
            }
        }
    }
    out
}

/// The two `ulCodePageRange` words.
///
/// A direct port of the FontForge rule that `fontTools.calcCodePageRanges`
/// also follows: a code page is claimed when a character only that page has is
/// present, plus, for the pages that need it, printable ASCII and the box
/// drawing that the DOS pages are recognized by. It is a heuristic — code page
/// coverage is not something a Unicode cmap states — but it is the heuristic
/// every other tool uses, so a font built here reports what a font built
/// elsewhere with the same coverage would.
pub(super) fn code_page_ranges(codepoints: &std::collections::HashSet<u32>) -> [u32; 2] {
    let has = |c: char| codepoints.contains(&(c as u32));
    let has_ascii = (0x20..0x7E).all(|c| codepoints.contains(&c));
    let has_lineart = has('┤');
    let has_root = has('√');

    let mut bits: u64 = 0;
    let mut set = |bit: u32| bits |= 1u64 << bit;

    if has('Þ') && has_ascii {
        set(0);
    } // Latin 1
    if has('Ľ') && has_ascii {
        set(1); // Latin 2: Eastern Europe
        if has_lineart {
            set(58);
        } // Latin 2
    }
    if has('Б') {
        set(2); // Cyrillic
        if has('Ѕ') && has_lineart {
            set(57);
        } // IBM Cyrillic
        if has('╜') && has_lineart {
            set(49);
        } // MS-DOS Russian
    }
    if has('Ά') {
        set(3); // Greek
        if has_lineart && has('½') {
            set(48);
        } // IBM Greek
        if has_lineart && has_root {
            set(60);
        } // Greek, former 437 G
    }
    if has('İ') && has_ascii {
        set(4); // Turkish
        if has_lineart {
            set(56);
        } // IBM Turkish
    }
    if has('א') {
        set(5); // Hebrew
        if has_lineart && has_root {
            set(53);
        } // Hebrew
    }
    if has('ر') {
        set(6); // Arabic
        if has_root {
            set(51);
        } // Arabic
        if has_lineart {
            set(61);
        } // Arabic; ASMO 708
    }
    if has('ŗ') && has_ascii {
        set(7); // Windows Baltic
        if has_lineart {
            set(59);
        } // MS-DOS Baltic
    }
    if has('₫') && has_ascii {
        set(8);
    } // Vietnamese
    if has('ๅ') {
        set(16);
    } // Thai
    if has('エ') {
        set(17);
    } // JIS/Japan
    if has('ㄅ') {
        set(18);
    } // Chinese: Simplified
    if has('ㄱ') {
        set(19);
    } // Korean Wansung
    if has('央') {
        set(20);
    } // Chinese: Traditional
    if has('곴') {
        set(21);
    } // Korean Johab
    if has('♥') && has_ascii {
        set(30);
    } // OEM
    if has('þ') && has_ascii && has_lineart {
        set(54);
    } // MS-DOS Icelandic
    if has('╚') && has_ascii {
        set(62);
        set(63);
    } // WE/Latin 1, US
    if has_ascii && has_lineart && has_root {
        if has('Å') {
            set(50);
        } // MS-DOS Nordic
        if has('é') {
            set(52);
        } // MS-DOS Canadian French
        if has('õ') {
            set(55);
        } // MS-DOS Portuguese
    }
    if has_ascii && has('‰') && has('∑') {
        set(29);
    } // Macintosh (US Roman)

    [bits as u32, (bits >> 32) as u32]
}
