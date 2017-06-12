use std::borrow::Cow;
use std::collections::{HashMap, BTreeMap, BTreeSet};
use rmp::decode;

fn read_str<'a>(buf: &mut &'a [u8]) -> Result<&'a str, decode::DecodeStringError<'a>> {
    let (s, buf_) = decode::read_str_from_slice(*buf)?;
    *buf = buf_;
    Ok(s)
}

#[derive(Debug)]
struct Row {
    name: &'static str,
}

#[derive(Debug)]
pub struct UnicodeData {
    chars: HashMap<u32, Row>,
    char_ranges: Vec<(u32, u32, Row)>,
}

impl UnicodeData {
    fn new(data: &'static [u8]) -> UnicodeData {
        let mut buf = data;
        let mut chars = HashMap::new();
        let mut char_ranges = Vec::new();
        loop {
            let cp: i64 = decode::read_int(&mut buf).unwrap();
            if cp < 0 { break; }

            let name = read_str(&mut buf).unwrap();
            let row = Row { name: name };

            let firstcp = (cp >> 32) as u32;
            let lastcp = (cp & 0xffffffff) as u32;
            if firstcp != 0 {
                char_ranges.push((firstcp, lastcp, row));
            } else {
                chars.insert(lastcp, row);
            }
        }
        UnicodeData { chars: chars, char_ranges: char_ranges }
    }

    fn row(&self, cp: u32) -> Option<&Row> {
        for &(lo, hi, ref row) in &self.char_ranges {
            if lo <= cp && cp <= hi { return Some(row); }
        }
        self.chars.get(&cp)
    }

    pub fn name(&self, cp: u32) -> Cow<'static, str> {
        if let Some(row) = self.row(cp) {
            if row.name.starts_with("\x01") { // NR1: hangul syllable
                let cp = cp - 0xac00;
                let a = (cp / 588) as usize;
                let b = (cp / 28 % 21) as usize;
                let c = (cp % 28) as usize;
                Cow::from(format!("{}{}{}{}",
                    &row.name[1..],
                    ["G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "",
                     "J", "JJ", "C", "K", "T", "P", "H"][a],
                    ["A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE",
                     "OE", "YO", "U", "WEO", "WE", "WI", "YU", "EU", "YI", "I"][b],
                    ["", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB",
                     "LS", "LT", "LP", "LH", "M", "B", "BS", "S", "SS", "NG", "J",
                     "C", "K", "T", "P", "H"][c],
                ))
            } else if row.name.starts_with("\x02") { // NR2: ideograph
                Cow::from(format!("{}{:04X}", &row.name[1..], cp))
            } else {
                Cow::from(row.name)
            }
        } else {
            Cow::from("")
        }
    }
}

#[derive(Debug, Clone)]
pub struct UdhrSample {
    pub code: &'static str,
    pub text: &'static str,
    pub required_chars: BTreeSet<char>,
}

lazy_static! {
    pub static ref UNICODE_DATA: UnicodeData = {
        UnicodeData::new(include_bytes!(concat!(env!("OUT_DIR"), "/unicode_data.dat")))
    };

    pub static ref UDHR_SAMPLES: Vec<UdhrSample> = {
        let mut udhr = include!(concat!(env!("OUT_DIR"), "/udhr.rs")).to_owned();

        // prioritize some languages to optimize the number of unique languages
        let reorder = ["eng", "rus", "kor"];
        udhr.sort_by_key(|&(ref code, _, _)| (!reorder.contains(&&code[..]), code.to_owned()));

        let mut samples = Vec::new();
        let mut appeared: BTreeSet<char> = BTreeSet::new();
        for (code, text, required_chars) in udhr {
            let required_chars: BTreeSet<_> = required_chars.chars().collect();
            let prevlen = appeared.len();
            appeared.extend(&required_chars);
            if appeared.len() > prevlen { // has new code points
                samples.push(UdhrSample {
                    code: code, text: text, required_chars: required_chars,
                });
            }
        }
        samples
    };

    pub static ref CONFUSABLES: BTreeMap<&'static str, &'static str> = {
        let mut buf = &include_bytes!(concat!(env!("OUT_DIR"), "/confusables.dat"))[..];
        let mut map = BTreeMap::new();
        for _ in 0..decode::read_map_len(&mut buf).unwrap() {
            let key = read_str(&mut buf).unwrap();
            let value  = read_str(&mut buf).unwrap();
            map.insert(key, value);
        }
        map
    };
}

