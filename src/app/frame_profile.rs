//! UI-thread frame times for one editing scenario against the real `font/`.
//!
//! `#[ignore]`d, like the profiling harness in `ref_composite/`: it reads
//! `font/`, which no test may otherwise read, and what it reports is a table of
//! timings to read rather than an assertion. Run it with
//!
//! ```sh
//! cargo test -r frame_profile -- --ignored --nocapture
//! ```
//!
//! The scenario is the one that made the editor visibly stutter: two
//! commented-out lines of a large file are uncommented, the second is cleared,
//! and a `ref` is typed into it one character at a time with a pause after
//! each, while the background rebuilds land in between. Each frame is timed the
//! way eframe spends it on the UI thread — `update()` plus the tessellation
//! that follows it — and the pauses matter: a rebuild that costs a millisecond
//! run back to back costs several after the idle frames between two keystrokes,
//! because by then its memory is cold.
//!
//! The numbers are the Mac's. The machine the editor is meant for is about three
//! times slower, so a frame here should stay well under a third of its budget.

use super::*;
use std::time::{Duration, Instant};

const FILE: &str = "han-0001.unf";
const HEADER: &str = "// glyph han-9fd6:15x16 15 16 // 鿖";
const TYPED: &str = "ref han-5408:15x16";
const FRAME: Duration = Duration::from_millis(16);

struct Driver {
    ctx: egui::Context,
    app: UniformApp,
    frames: Vec<(String, Duration)>,
    label: String,
}

impl Driver {
    fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Duration {
        let started = Instant::now();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1000.0),
            )),
            modifiers,
            events,
            ..Default::default()
        };
        let output = self.ctx.run(input, |ctx| self.app.frame(ctx));
        drop(self.ctx.tessellate(output.shapes, output.pixels_per_point));
        let took = started.elapsed();
        self.frames.push((self.label.clone(), took));
        took
    }

    /// Frames at 60 Hz for `span` of wall time, as a repainting window would.
    fn idle(&mut self, span: Duration) {
        let until = Instant::now() + span;
        while Instant::now() < until {
            let took = self.frame(vec![], Default::default());
            std::thread::sleep(FRAME.saturating_sub(took));
        }
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        let event = egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        self.frame(vec![event], modifiers);
    }

    fn text(&mut self, s: &str) {
        self.frame(vec![egui::Event::Text(s.to_string())], Default::default());
    }

    fn line(&self, idx: usize) -> String {
        let doc = self.app.active_doc().unwrap();
        doc.lines[idx].as_text().unwrap_or("<grid>").to_string()
    }
}

fn settled(app: &UniformApp) -> bool {
    app.named_glyphs_gen == app.font_build_gen && app.font_data.is_some() && !app.rebuild_inflight
}

#[test]
#[ignore]
fn frame_profile() {
    // Not taken by the release binary, so not timed here.
    crate::editor::harness::SKIP_SNAPSHOT.set(true);
    // The real UI thread runs at user-interactive QoS. A test thread left at
    // the default is moved to the efficiency cores between frames, and every
    // number below would be the E-cores'. A measurement fix only: nothing in
    // the editor depends on it.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
    // As `main` does for the GUI.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    crate::heap::purge_off_the_ui_thread();

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("font");
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(2.0);
    let app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.clone()));
    let mut d = Driver {
        ctx,
        app,
        frames: Vec::new(),
        label: "startup".into(),
    };

    d.app.open_file(dir.join(FILE));
    let deadline = Instant::now() + Duration::from_secs(120);
    while !settled(&d.app) {
        assert!(Instant::now() < deadline, "the first build never landed");
        d.idle(Duration::from_millis(50));
    }
    d.idle(Duration::from_millis(500));

    let header = {
        let doc = d.app.active_doc().unwrap();
        doc.lines
            .iter()
            .position(|l| l.as_text() == Some(HEADER))
            .expect("the scenario's header line")
    };
    let state = &mut d.app.active_doc_mut().unwrap().editor_state;
    state.goto_line(header);
    state.refocus();
    d.idle(Duration::from_millis(300));

    d.label = "uncomment".into();
    d.key(egui::Key::ArrowDown, egui::Modifiers::SHIFT);
    d.key(egui::Key::End, egui::Modifiers::SHIFT);
    d.key(egui::Key::Slash, egui::Modifiers::COMMAND);
    d.idle(Duration::from_millis(1500));
    // The header now draws an (empty) grid, so the IDC line is two below it.
    let idc = header + 2;
    assert!(d.line(idc).starts_with("⿱ han-5408"), "{:?}", d.line(idc));

    d.label = "clear".into();
    d.app.active_doc_mut().unwrap().editor_state.goto_line(idc);
    d.key(egui::Key::Home, Default::default());
    d.key(egui::Key::End, egui::Modifiers::SHIFT);
    d.key(egui::Key::Backspace, Default::default());
    d.idle(Duration::from_millis(1500));

    for (i, ch) in TYPED.char_indices() {
        d.label = format!("type {:>2} {:?}", i, &TYPED[..i + ch.len_utf8()]);
        d.text(&ch.to_string());
        d.idle(Duration::from_millis(150));
    }
    d.label = "settle".into();
    d.idle(Duration::from_millis(3000));
    assert_eq!(d.line(idc), TYPED);

    report(&d.frames);
}

fn report(frames: &[(String, Duration)]) {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut labels: Vec<&str> = Vec::new();
    for (l, _) in frames {
        if !labels.contains(&l.as_str()) {
            labels.push(l);
        }
    }
    eprintln!(
        "{:<28} {:>5} {:>8} {:>8} {:>8} {:>5}",
        "phase", "n", "mean", "max", "first", ">16"
    );
    let mut all = Vec::new();
    for label in labels {
        if label == "startup" {
            continue;
        }
        let times: Vec<Duration> = frames
            .iter()
            .filter(|(l, _)| l == label)
            .map(|(_, t)| *t)
            .collect();
        all.extend(&times);
        let total: Duration = times.iter().sum();
        eprintln!(
            "{:<28} {:>5} {:>8.2} {:>8.2} {:>8.2} {:>5}",
            label,
            times.len(),
            ms(total) / times.len() as f64,
            ms(*times.iter().max().unwrap()),
            ms(times[0]),
            times.iter().filter(|t| **t > FRAME).count(),
        );
    }
    all.sort();
    let total: Duration = all.iter().sum();
    eprintln!(
        "all: n={} mean={:.2} p50={:.2} p90={:.2} p99={:.2} max={:.2} >16ms={} >5ms={}",
        all.len(),
        ms(total) / all.len() as f64,
        ms(all[all.len() / 2]),
        ms(all[all.len() * 9 / 10]),
        ms(all[all.len() * 99 / 100]),
        ms(*all.last().unwrap()),
        all.iter().filter(|t| **t > FRAME).count(),
        all.iter()
            .filter(|t| **t > Duration::from_millis(5))
            .count(),
    );
}
