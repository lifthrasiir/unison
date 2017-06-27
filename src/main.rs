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
extern crate fpa;
extern crate cast;
extern crate console;
extern crate indicatif;

use std::io;
use std::fs::File;
use std::sync::Arc;
use std::thread;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};

pub mod external_data;
pub mod font;
pub mod contour;
pub mod sample;

fn main() {
    let progress = Arc::new(MultiProgress::new());

    let font = font::Font::from_json(io::stdin()).expect("failed to parse");

    fn set_style(bar: ProgressBar) -> ProgressBar {
        bar.set_style(
            ProgressStyle::default_bar().template("{prefix} {wide_bar} {pos}/{len} ETA {eta}")
        );
        bar
    }

    let pgm_sample_bar = set_style(progress.add(ProgressBar::new(1)));
    let html_sample_bar = set_style(progress.add(ProgressBar::new(1)));
    let live_sample_bar = set_style(progress.add(ProgressBar::new(1)));

    let progress_thread = {
        let progress = progress.clone();
        thread::spawn(move || progress.join_and_clear().unwrap())
    };

    rayon::scope(|s| {
        s.spawn(|_| {
            let mut f = File::create("sample.pgm").expect("failed to open PGM");
            pgm_sample_bar.set_prefix("PGM");
            sample::write_pgm(&mut f, &font, &pgm_sample_bar).expect("failed to write PGM");
            pgm_sample_bar.finish();
        });

        s.spawn(|_| {
            let mut f = File::create("sample.html").expect("failed to open HTML");
            html_sample_bar.set_prefix("HTML");
            sample::write_html(&mut f, &font, &html_sample_bar).expect("failed to write HTML");
            html_sample_bar.finish();
        });

        s.spawn(|_| {
            let mut f = File::create("live.html").expect("failed to open HTML");
            live_sample_bar.set_prefix("Live example");
            sample::write_live_html(&mut f, &font, &live_sample_bar).expect("failed to write HTML");
            live_sample_bar.finish();
        });
    });

    progress_thread.join().unwrap();
}

