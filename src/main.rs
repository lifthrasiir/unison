extern crate serde;
#[macro_use] extern crate serde_derive;
extern crate serde_json;
extern crate rayon;
extern crate base64;
extern crate gif;
#[macro_use] extern crate lazy_static;
extern crate rmp;
extern crate unicode_normalization;
extern crate md5;

use std::io;
use std::fs::File;

pub mod external_data;
pub mod font;
pub mod contour;
pub mod sample;

fn main() {
    let font = font::Font::from_json(io::stdin()).expect("failed to parse");

    rayon::scope(|s| {
        s.spawn(|_| {
            let mut f = File::create("sample.pgm").expect("failed to open PGM");
            sample::write_pgm(&mut f, &font).expect("failed to write PGM");
        });

        s.spawn(|_| {
            let mut f = File::create("sample.html").expect("failed to open HTML");
            sample::write_html(&mut f, &font).expect("failed to write HTML");
        });

        s.spawn(|_| {
            let mut f = File::create("live.html").expect("failed to open HTML");
            sample::write_live_html(&mut f, &font).expect("failed to write HTML");
        });
    });
}

