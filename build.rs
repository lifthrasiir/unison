extern crate rmp;
extern crate zip;
extern crate xml;

use std::u32;
use std::char;
use std::env;
use std::num;
use std::path::Path;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::collections::{BTreeMap, BTreeSet};
use rmp::encode;
use zip::ZipArchive;
use xml::reader::{EventReader, XmlEvent};

struct Error(io::Error);

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error { Error(e) }
}

macro_rules! map_errors {
    ($($t:ty => $v:ident;)*) => ($(    
        impl From<$t> for Error {
            fn from(e: $t) -> Error { Error(io::Error::new(io::ErrorKind::$v, e)) }
        }
    )*)
}

map_errors! {
    &'static str => InvalidData;
    num::ParseIntError => InvalidData;
    encode::ValueWriteError => Other;
    zip::result::ZipError => InvalidData;
    xml::reader::Error => InvalidData;
}

fn parse_scsv_line(line: io::Result<String>) -> Option<io::Result<Vec<String>>> {
    match line {
        Ok(line) => {
            let mut line = &line[..];
            if let Some(sep) = line.find('#') {
                line = &line[..sep];
            }
            let line = line.trim().trim_matches('\u{feff}');
            if line.is_empty() {
                None
            } else {
                Some(Ok(line.split(';').map(|c| c.trim().to_owned()).collect()))
            }
        },
        Err(e) => Some(Err(e)),
    }
}

fn build_unicode_data(data_dir: &Path, out_dir: &Path) -> Result<(), Error> {
    let f = BufReader::new(File::open(&data_dir.join("UnicodeData.txt"))?);
    let mut out = File::create(&out_dir.join("unicode_data.dat"))?;

    let mut range = None;
    for columns in f.lines().filter_map(parse_scsv_line) {
        let columns = columns?;
        if columns.len() != 15 {
            return Err(Error::from("invalid number of columns"));
        }
        let mut cp = u32::from_str_radix(&columns[0], 16)? as i64;
        let mut name = &columns[1][..];
        if name.starts_with("<") {
            if name == "<control>" {
                name = "";
            } else if name.ends_with(", First>") {
                if range.is_some() {
                    return Err(Error::from("mismatching range"));
                }
                let ident = &name[1..name.len()-8];
                range = Some((ident.to_owned(), cp));
                continue;
            } else if name.ends_with(", Last>") {
                let ident = &name[1..name.len()-7];
                if let Some((firstident, firstcp)) = range {
                    if firstident != ident {
                        return Err(Error::from("mismatching range"));
                    }
                    if firstcp >= cp {
                        return Err(Error::from("invalid range"));
                    }
                    cp |= (firstcp as i64) << 32;
                    range = None;
                } else {
                    return Err(Error::from("mismatching range"));
                }
                let newname = match ident {
                    "CJK Ideograph" |
                        "CJK Ideograph Extension A" |
                        "CJK Ideograph Extension B" |
                        "CJK Ideograph Extension C" |
                        "CJK Ideograph Extension D" |
                        "CJK Ideograph Extension E" |
                        "CJK Ideograph Extension F" => "\x02CJK UNIFIED IDEOGRAPH-",
                    "Tangut Ideograph" => "\x02TANGUT IDEOGRAPH-",
                    "Hangul Syllable" => "\x01HANGUL SYLLABLE ",
                    "Plane 15 Private Use" |
                        "Plane 16 Private Use" |
                        "Non Private Use High Surrogate" |
                        "Private Use High Surrogate" |
                        "Low Surrogate" |
                        "Private Use" => "",
                    _ => return Err(Error::from("invalid range identifier")),
                };
                name = newname;
            } else {
                return Err(Error::from("mismatching range"));
            }
        } else if range.is_some() {
            return Err(Error::from("mismatching range"));
        }
        encode::write_sint(&mut out, cp)?;
        encode::write_str(&mut out, name)?;
    }
    if range.is_some() {
        return Err(Error::from("mismatching range"));
    }
    encode::write_sint(&mut out, -1)?;

    Ok(())
}

fn build_udhr(data_dir: &Path, out_dir: &Path) -> Result<(), Error> {
    let mut zip = ZipArchive::new(File::open(&data_dir.join("udhr_xml.zip"))?)?;
    let mut samples = Vec::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i)?;
        let name = file.name().to_owned();
        if name.starts_with("udhr_") && name.ends_with(".xml") {
            let code = name[5..name.len()-4].to_owned();

            let mut depth = 0;
            let mut article_depth = None;
            let mut first_para_depth = None;
            let mut first_para_found = false;
            let mut first_para = String::new();
            let mut chars = BTreeSet::new();
            for ev in EventReader::new(file) {
                match ev? {
                    XmlEvent::StartElement { name, attributes, .. } => {
                        depth += 1;
                        if article_depth.is_none() && name.local_name == "article" &&
                                attributes.iter().any(|a| a.name.local_name == "number" &&
                                                          a.value == "1") {
                            article_depth = Some(depth);
                        }
                        if !first_para_found && first_para_depth.is_none() &&
                                article_depth.is_some() && name.local_name == "para" {
                            first_para_found = true;
                            first_para_depth = Some(depth);
                        }
                    }
                    XmlEvent::EndElement { .. } => {
                        depth -= 1;
                        if article_depth.map_or(false, |v| depth < v) {
                            article_depth = None;
                        }
                        if first_para_depth.map_or(false, |v| depth < v) {
                            break;
                        }
                    }
                    XmlEvent::CData(ref s) |
                    XmlEvent::Characters(ref s) |
                    XmlEvent::Whitespace(ref s) => {
                        for c in s.chars() {
                            if c != '\t' && c != '\r' && c != '\n' { chars.insert(c); }
                        }
                        if first_para_depth.is_some() {
                            first_para.push_str(&s.replace(&['\t', '\r', '\n'][..], " "));
                        }
                    }
                    _ => {}
                }
            }

            if first_para_found {
                let chars: String = chars.into_iter().collect();
                samples.push((code, first_para, chars));
            }
        }
    }
    drop(zip);

    let mut out = File::create(&out_dir.join("udhr.rs"))?;
    write!(&mut out, "{:?}", samples)?;
    Ok(())
}

fn build_confusables(data_dir: &Path, out_dir: &Path) -> Result<(), Error> {
    let f = BufReader::new(File::open(&data_dir.join("confusables.txt"))?);
    let mut map = BTreeMap::new();
    for columns in f.lines().filter_map(parse_scsv_line) {
        let columns = columns?;
        if columns.len() != 3 {
            return Err(Error::from("invalid number of columns"));
        }
        let cp = u32::from_str_radix(&columns[0], 16)?;
        let target = columns[1].split_whitespace();
        let target = target.map(|s| u32::from_str_radix(s, 16)).collect::<Result<Vec<_>, _>>()?;
        map.entry(target).or_insert(Vec::new()).push(cp);
    }

    let mut out = File::create(&out_dir.join("confusables.dat"))?;
    encode::write_map_len(&mut out, map.len() as u32)?;
    for (target, cps) in map {
        let target: Option<String> = target.into_iter().map(|c| char::from_u32(c)).collect();
        let cps: Option<String> = cps.into_iter().map(|c| char::from_u32(c)).collect();
        if let (Some(target), Some(cps)) = (target, cps) {
            encode::write_str(&mut out, &target)?;
            encode::write_str(&mut out, &cps)?;
        } else {
            return Err(Error::from("invalid mapping"));
        }
    }
    Ok(())
}

pub fn main() {
    let data_dir = Path::new(&env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("data");
    let out_dir = Path::new(&env::var_os("OUT_DIR").unwrap()).to_owned();

    build_unicode_data(&data_dir, &out_dir).map_err(|e| e.0).expect("build_unicode_data failed");
    println!("cargo:rerun-if-changed=data/UnicodeData.txt");

    build_udhr(&data_dir, &out_dir).map_err(|e| e.0).expect("build_udhr failed");
    println!("cargo:rerun-if-changed=data/udhr_xml.zip");

    build_confusables(&data_dir, &out_dir).map_err(|e| e.0).expect("build_confusables failed");
    println!("cargo:rerun-if-changed=data/confusables.txt");
}

