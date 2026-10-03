use super::*;

fn coverage(w: usize, h: usize, contour: &[(f32, f32)]) -> Vec<f32> {
    let mut cov = Coverage::new(w, h);
    let polygons = Polygons {
        contours: vec![contour.to_vec()],
        ..Default::default()
    };
    cov.fill(&polygons, |p| p);
    cov.finish()
}

#[test]
fn coverage_of_an_aligned_square_is_exact() {
    let cov = coverage(4, 4, &[(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)]);
    for y in 0..4 {
        for x in 0..4 {
            let inside = (1..3).contains(&x) && (1..3).contains(&y);
            assert_eq!(cov[y * 4 + x], if inside { 1.0 } else { 0.0 }, "({x}, {y})");
        }
    }
}

#[test]
fn coverage_winding_does_not_depend_on_direction() {
    let cw = coverage(2, 2, &[(0.0, 0.0), (2.0, 0.0), (0.0, 2.0)]);
    let ccw = coverage(2, 2, &[(0.0, 0.0), (0.0, 2.0), (2.0, 0.0)]);
    assert_eq!(cw, ccw);
    // The diagonal halves the two cells it cuts.
    assert_eq!(cw, vec![1.0, 0.5, 0.5, 0.0]);
}

#[test]
fn coverage_clamps_ink_left_of_the_canvas() {
    // A square hanging off the left edge still inks the column it reaches.
    let cov = coverage(2, 1, &[(-1.0, 0.0), (1.0, 0.0), (1.0, 1.0), (-1.0, 1.0)]);
    assert_eq!(cov, vec![1.0, 0.0]);
}

const SOURCE: &str = "\
meta height 4
meta ascent 3
meta descent 1

glyph slope 4 4
........
..../1@@
../1@@0/
..@@0/..
map A = slope

glyph unmapped 1 4
..
@@
@@
..

glyph also-unmapped = unmapped
";

fn ascii(items: Vec<Item>) -> Result<String, String> {
    let doc = crate::document_io::parse_document_from_str(SOURCE, "test.unf".into()).unwrap();
    let opts = Options {
        input: PathBuf::new(),
        face: None,
        output: None,
        zoom: 4,
        ascii: true,
        items,
    };
    render(&[doc], &opts)
}

#[test]
fn a_glyph_prints_its_bitmap_build() {
    // A lit half-cell is a whole pixel there; an unlit one is nothing.
    assert_eq!(
        ascii(vec![Item::Glyph("slope".into())]).unwrap(),
        "slope: advance 4, x 0..4, y -1..3 (baseline under row 2)\n\
         ........\n\
         ....@@@@\n\
         ..@@@@..\n\
         ..@@....\n\n",
    );
}

#[test]
fn a_glyph_nothing_maps_is_drawn_all_the_same() {
    let out = ascii(vec![Item::Glyph("unmapped".into())]).unwrap();
    assert_eq!(
        out,
        "unmapped: advance 1, x 0..1, y -1..3 (baseline under row 2)\n..\n@@\n@@\n..\n\n"
    );
}

#[test]
fn an_alias_is_drawn_as_its_target() {
    let out = ascii(vec![Item::Glyph("also-unmapped".into())]).unwrap();
    assert_eq!(
        out,
        "also-unmapped: advance 1, x 0..1, y -1..3 (baseline under row 2)\n..\n@@\n@@\n..\n\n"
    );
}

#[test]
fn an_on_demand_shape_is_drawn() {
    let out = ascii(vec![Item::Glyph("2x2-dl".into())]).unwrap();
    assert!(out.contains("@@..\n@@@@\n"), "{out}");
}

#[test]
fn text_is_shaped_and_laid_out() {
    let out = ascii(vec![Item::Text("AA".into())]).unwrap();
    assert!(
        out.starts_with("\"AA\" = slope slope: advance 8, x 0..8"),
        "{out}"
    );
    assert!(
        out.contains("....@@@@....@@@@\n..@@@@....@@@@..\n"),
        "{out}"
    );
}

#[test]
fn an_unknown_name_is_an_error() {
    let err = ascii(vec![Item::Glyph("nonesuch".into())]).unwrap_err();
    assert!(err.contains("nonesuch"), "{err}");
}

#[test]
fn arguments_need_something_to_draw_and_somewhere_to_put_it() {
    let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
    assert!(parse_args(&args("-i d --ascii")).is_err());
    assert!(parse_args(&args("-i d -g a")).is_err());
    assert!(parse_args(&args("-i d -g a --zoom 0 --ascii")).is_err());
    let opts = parse_args(&args("-i d -g a -t b -o x.png")).unwrap();
    assert_eq!(
        opts.items,
        vec![Item::Glyph("a".into()), Item::Text("b".into())]
    );
}
