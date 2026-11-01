//! Reading hOCR and ALTO into an [`OcrResult`].
//!
//! For engines and modules that already produce one of the standard
//! formats (Tesseract writes hOCR and ALTO; many cloud services and
//! digitisation tools write ALTO). The readers take the subset that maps
//! onto Glossa's tree and ignore the rest:
//!
//! * hOCR: `ocr_page` (its `bbox` is the image size), `ocr_carea` /
//!   `ocr_column` (blocks), `ocr_par`, `ocr_line` and its kin
//!   (`ocr_caption`, `ocr_header`, `ocr_textfloat`, `ocrx_line`), and
//!   `ocrx_word`; the `bbox`, `baseline`, `textangle`, `poly` and `x_wconf`
//!   properties; `lang` and `dir` attributes.
//! * ALTO (any version): `Page` `WIDTH`/`HEIGHT`/`LANG`, `TextBlock`,
//!   `TextLine` (with `BASELINE` as a number or as points), `String`
//!   (`CONTENT`, `WC`, `LANG`), `ROTATION`, `BASEDIRECTION` and
//!   `Shape/Polygon`. Coordinates are taken as pixels; ALTO files in other
//!   measurement units keep their numbers.
//!
//! Both readers are tolerant (HTML-style hOCR, missing end tags, unknown
//! elements) and never panic on bad input; they fail only when the document
//! holds nothing they recognise.

use super::xml::{tokenize, Event};
use super::{quad_geometry, unrotated_box, Baseline, OcrBlock, OcrLevel, OcrResult, Rect, WritingDirection};
use crate::lang::LangId;

/// hOCR or ALTO, whichever `input` is.
pub fn any(input: &str) -> Result<OcrResult, String> {
    let head: String = input.chars().take(4096).collect();
    if head.contains("<alto") || head.contains(":alto") || head.contains("TextBlock") {
        alto(input)
    } else {
        hocr(input)
    }
}

struct Frame {
    name: String,
    node: Option<OcrBlock>,
    text: String,
}

/// The tree being built: open elements and finished top-level nodes.
#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    top: Vec<OcrBlock>,
}

impl Builder {
    fn open(&mut self, name: String, node: Option<OcrBlock>) {
        self.stack.push(Frame { name, node, text: String::new() });
    }

    fn text(&mut self, t: &str) {
        if let Some(f) = self.stack.iter_mut().rev().find(|f| f.node.is_some()) {
            f.text.push_str(t);
        }
    }

    fn close(&mut self, name: &str) {
        if let Some(at) = self.stack.iter().rposition(|f| f.name.eq_ignore_ascii_case(name)) {
            while self.stack.len() > at {
                self.close_top();
            }
        }
    }

    fn close_top(&mut self) {
        let Some(f) = self.stack.pop() else { return };
        let Some(mut node) = f.node else { return };
        if node.children.is_empty() {
            if node.text.is_empty() {
                node.text = f.text.split_whitespace().collect::<Vec<_>>().join(" ");
            }
        } else {
            let sep = if node.level == Some(OcrLevel::Line) { " " } else { "\n" };
            node.text =
                node.children.iter().map(|c| c.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(sep);
        }
        match self.stack.iter_mut().rev().find_map(|f| f.node.as_mut()) {
            Some(parent) => parent.children.push(node),
            None => self.top.push(node),
        }
    }

    fn finish(mut self) -> Vec<OcrBlock> {
        while !self.stack.is_empty() {
            self.close_top();
        }
        self.top
    }

    /// The innermost node being built.
    fn current(&mut self) -> Option<&mut OcrBlock> {
        self.stack.iter_mut().rev().find_map(|f| f.node.as_mut())
    }
}

const VOID: &[&str] = &["br", "meta", "img", "link", "hr", "input", "col", "area", "base", "wbr"];

fn num(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Sets `bounds`, `angle` and `polygon` from an axis-aligned box, a
/// counter-clockwise rotation in degrees and an outline.
fn place(node: &mut OcrBlock, aabb: Option<Rect>, ccw_degrees: Option<f32>, polygon: Vec<[f32; 2]>) {
    let angle = ccw_degrees.map(|d| -d.to_radians()).filter(|a| *a != 0.0).unwrap_or(0.0);
    if let [a, b, c, d] = polygon.as_slice() {
        let (r, quad_angle) = quad_geometry(&[*a, *b, *c, *d]);
        node.bounds = Some(r);
        node.angle = quad_angle;
    } else if let Some(r) = aabb {
        match unrotated_box(r, angle) {
            Some(level) => {
                node.bounds = Some(level);
                node.angle = angle;
            }
            None => node.bounds = Some(r),
        }
    }
    node.polygon = polygon;
}

fn points(s: &str) -> Vec<[f32; 2]> {
    let nums: Vec<f32> = s.split(|c: char| c == ',' || c.is_whitespace()).filter_map(num).collect();
    nums.as_chunks::<2>().0.to_vec()
}

fn direction(s: &str) -> Option<WritingDirection> {
    match s.trim().to_ascii_lowercase().as_str() {
        "rtl" => Some(WritingDirection::Rtl),
        "ltr" => Some(WritingDirection::Ltr),
        "ttb" => Some(WritingDirection::Ttb),
        _ => None,
    }
}

fn page_lang(code: &str) -> Option<LangId> {
    LangId::from_code(code.trim()).filter(|l| l.is_language())
}

fn result(blocks: Vec<OcrBlock>, size: Option<(u32, u32)>, lang: Option<LangId>) -> OcrResult {
    let r = OcrResult { blocks, image_size: size, detected_lang: lang, ..Default::default() }.with_levels();
    OcrResult { text: r.text_from_blocks(), ..r }
}

// -- hOCR ---------------------------------------------------------------------------

/// The `title` properties: `bbox 1 2 3 4; x_wconf 95; image "a;b.png"`.
fn title_props(title: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in title.chars().chain(std::iter::once(';')) {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ';' if !quoted => {
                let p = current.trim();
                if !p.is_empty() {
                    let (k, v) = p.split_once(char::is_whitespace).unwrap_or((p, ""));
                    out.push((k.to_owned(), v.trim().to_owned()));
                }
                current.clear();
            }
            c => current.push(c),
        }
    }
    out
}

fn hocr_level(classes: &str) -> Option<OcrLevel> {
    let mut level = None;
    for c in classes.split_whitespace() {
        let l = match c {
            "ocr_carea" | "ocr_column" | "ocrx_block" => OcrLevel::Block,
            "ocr_par" => OcrLevel::Paragraph,
            "ocr_line" | "ocrx_line" | "ocr_caption" | "ocr_header" | "ocr_textfloat" => OcrLevel::Line,
            "ocrx_word" | "ocr_word" => OcrLevel::Word,
            _ => continue,
        };
        level = Some(l);
    }
    level
}

/// Read an hOCR document (the first `ocr_page`, or the whole document when
/// it has none).
pub fn hocr(input: &str) -> Result<OcrResult, String> {
    let mut b = Builder::default();
    let mut size = None;
    let mut lang = None;
    let mut pages = 0;
    for ev in tokenize(input) {
        match ev {
            Event::Start { name, attrs, empty } => {
                let lower = name.to_ascii_lowercase();
                if VOID.contains(&lower.as_str()) {
                    continue;
                }
                if lower == "html" {
                    lang = Event::attr(&attrs, "lang").and_then(page_lang);
                }
                let classes = Event::attr(&attrs, "class").unwrap_or("");
                let props = Event::attr(&attrs, "title").map(title_props).unwrap_or_default();
                let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                let bbox = prop("bbox").and_then(|v| {
                    let n: Vec<f32> = v.split_whitespace().filter_map(num).collect();
                    (n.len() == 4).then(|| Rect::from_corners(n[0], n[1], n[2], n[3]))
                });
                if classes.split_whitespace().any(|c| c == "ocr_page") {
                    pages += 1;
                    if pages > 1 {
                        // One page per result: stop at the second.
                        break;
                    }
                    size = bbox.map(|r| (r.width.max(0.0) as u32, r.height.max(0.0) as u32)).filter(|s| s.0 > 0);
                }
                let node = hocr_level(classes).map(|level| {
                    let mut n = OcrBlock { level: Some(level), ..Default::default() };
                    let poly = prop("poly").map(points).unwrap_or_default();
                    place(&mut n, bbox, prop("textangle").and_then(num), poly);
                    n.confidence = prop("x_wconf").and_then(num).map(|c| (c / 100.0).clamp(0.0, 1.0));
                    n.baseline = prop("baseline").and_then(|v| {
                        let n: Vec<f32> = v.split_whitespace().filter_map(num).collect();
                        match n.as_slice() {
                            [slope, offset, ..] => Some(Baseline { slope: *slope, offset: *offset }),
                            [offset] => Some(Baseline { slope: 0.0, offset: *offset }),
                            [] => None,
                        }
                    });
                    n.lang = Event::attr(&attrs, "lang").filter(|l| !l.is_empty()).map(str::to_owned);
                    n.direction = Event::attr(&attrs, "dir").and_then(direction);
                    if Event::attr(&attrs, "style")
                        .is_some_and(|s| s.replace(' ', "").contains("writing-mode:vertical"))
                    {
                        n.direction = Some(WritingDirection::Ttb);
                    }
                    n
                });
                b.open(name, node);
                if empty {
                    b.close_top();
                }
            }
            Event::End { name } => b.close(&name),
            Event::Text(t) => b.text(&t),
        }
    }
    let blocks = b.finish();
    if pages == 0 && blocks.is_empty() {
        return Err("not hOCR: no ocr_page, ocr_line or ocrx_word elements".into());
    }
    Ok(result(blocks, size, lang))
}

// -- ALTO ---------------------------------------------------------------------------

/// Read an ALTO document (any version; the first `Page`).
pub fn alto(input: &str) -> Result<OcrResult, String> {
    let mut b = Builder::default();
    let mut size = None;
    let mut lang = None;
    let mut pages = 0;
    let mut seen_alto = false;
    for ev in tokenize(input) {
        match ev {
            Event::Start { name, attrs, empty } => {
                let a = |k: &str| Event::attr(&attrs, k);
                let f = |k: &str| a(k).and_then(num);
                let aabb = match (f("HPOS"), f("VPOS"), f("WIDTH"), f("HEIGHT")) {
                    (Some(x), Some(y), Some(w), Some(h)) => Some(Rect::new(x, y, w.max(0.0), h.max(0.0))),
                    _ => None,
                };
                let level = match name.as_str() {
                    "alto" => {
                        seen_alto = true;
                        None
                    }
                    "Page" => {
                        pages += 1;
                        if pages > 1 {
                            break;
                        }
                        size = match (f("WIDTH"), f("HEIGHT")) {
                            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some((w as u32, h as u32)),
                            _ => None,
                        };
                        lang = a("LANG").and_then(page_lang);
                        None
                    }
                    "TextBlock" => Some(OcrLevel::Block),
                    "TextLine" => Some(OcrLevel::Line),
                    "String" => Some(OcrLevel::Word),
                    "Polygon" => {
                        let poly = a("POINTS").map(points).unwrap_or_default();
                        if let Some(n) = b.current() {
                            if n.polygon.is_empty() && poly.len() >= 3 {
                                let aabb = n.bounds;
                                place(n, aabb, None, poly);
                            }
                        }
                        None
                    }
                    _ => None,
                };
                let node = level.map(|level| {
                    let mut n = OcrBlock { level: Some(level), ..Default::default() };
                    place(&mut n, aabb, f("ROTATION"), Vec::new());
                    n.lang = a("LANG").or(a("language")).filter(|l| !l.is_empty()).map(str::to_owned);
                    n.direction = a("BASEDIRECTION").and_then(direction);
                    if level == OcrLevel::Word {
                        n.text = a("CONTENT").unwrap_or("").to_owned();
                        n.confidence = f("WC").map(|c| c.clamp(0.0, 1.0));
                    }
                    if let (OcrLevel::Line, Some(base), Some(r)) = (level, a("BASELINE"), aabb) {
                        let bottom = r.y + r.height;
                        let pts = points(base);
                        n.baseline = match pts.as_slice() {
                            [] => num(base).map(|y| Baseline { slope: 0.0, offset: y - bottom }),
                            [p] => Some(Baseline { slope: 0.0, offset: p[1] - bottom }),
                            [p, .., q] => {
                                let dx = q[0] - p[0];
                                let slope = if dx.abs() > f32::EPSILON { (q[1] - p[1]) / dx } else { 0.0 };
                                Some(Baseline { slope, offset: p[1] + slope * (r.x - p[0]) - bottom })
                            }
                        };
                    }
                    n
                });
                b.open(name, node);
                if empty {
                    b.close_top();
                }
            }
            Event::End { name } => b.close(&name),
            Event::Text(_) => {}
        }
    }
    let blocks = b.finish();
    if !seen_alto && pages == 0 && blocks.is_empty() {
        return Err("not ALTO: no alto, Page or TextBlock elements".into());
    }
    Ok(result(blocks, size, lang))
}

#[cfg(test)]
mod tests {
    use super::super::export::{self, OcrFormat};
    use super::*;

    /// A page in the shape every engine's result takes after mapping: a
    /// block with a rotated line, a level line with words and confidences,
    /// and a right-to-left line.
    fn sample() -> OcrResult {
        let word = |t: &str, x: f32, c: f32| OcrBlock {
            bounds: Some(Rect::new(x, 20.0, 40.0, 12.0)),
            confidence: Some(c),
            ..OcrBlock::new(OcrLevel::Word, t)
        };
        OcrResult {
            text: "Hello world\nSkewed\nשלום".into(),
            image_size: Some((400, 300)),
            detected_lang: Some(LangId::ENGLISH),
            provider_id: "test".into(),
            blocks: vec![OcrBlock {
                bounds: Some(Rect::new(10.0, 10.0, 300.0, 200.0)),
                lang: Some("en".into()),
                children: vec![
                    OcrBlock {
                        bounds: Some(Rect::new(10.0, 20.0, 90.0, 12.0)),
                        baseline: Some(Baseline { slope: 0.0, offset: -3.0 }),
                        children: vec![word("Hello", 10.0, 0.96), word("world", 60.0, 0.5)],
                        ..OcrBlock::new(OcrLevel::Line, "Hello world")
                    },
                    OcrBlock {
                        bounds: Some(Rect::new(100.0, 100.0, 100.0, 20.0)),
                        angle: -std::f32::consts::FRAC_PI_6,
                        ..OcrBlock::new(OcrLevel::Line, "Skewed")
                    },
                    OcrBlock {
                        bounds: Some(Rect::new(200.0, 160.0, 60.0, 14.0)),
                        direction: Some(WritingDirection::Rtl),
                        lang: Some("he".into()),
                        ..OcrBlock::new(OcrLevel::Line, "שלום")
                    },
                ],
                ..OcrBlock::new(OcrLevel::Block, "Hello world\nSkewed\nשלום")
            }],
            ..Default::default()
        }
    }

    fn close(a: Option<Rect>, b: Option<Rect>, tol: f32) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => {
                (a.x - b.x).abs() <= tol
                    && (a.y - b.y).abs() <= tol
                    && (a.width - b.width).abs() <= tol
                    && (a.height - b.height).abs() <= tol
            }
            (None, None) => true,
            _ => false,
        }
    }

    /// The reader's tree against the original, node by node: text, level,
    /// box, rotation and confidence survive the trip.
    fn assert_same_tree(original: &OcrResult, read: &OcrResult, with_paragraphs: bool) {
        let flat = |r: &OcrResult| -> Vec<OcrBlock> {
            r.nodes().filter(|n| with_paragraphs || n.level != Some(OcrLevel::Paragraph)).cloned().collect()
        };
        let (a, b) = (flat(original), flat(read));
        assert_eq!(a.len(), b.len(), "node count\n{a:#?}\n{b:#?}");
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.level, y.level);
            assert_eq!(x.text, y.text);
            assert!(close(x.bounds, y.bounds, 1.0), "{:?} vs {:?}", x.bounds, y.bounds);
            assert!((x.angle - y.angle).abs() < 0.01, "angle {} vs {}", x.angle, y.angle);
            assert_eq!(x.confidence.map(|c| (c * 100.0).round()), y.confidence.map(|c| (c * 100.0).round()));
            assert_eq!(x.direction, y.direction);
            assert_eq!(x.lang, y.lang);
        }
    }

    #[test]
    fn hocr_round_trips() {
        let original = sample();
        let doc = export::hocr(&original);
        let read = hocr(&doc).unwrap();
        assert_eq!(read.image_size, Some((400, 300)));
        assert_eq!(read.detected_lang, Some(LangId::ENGLISH));
        assert_eq!(read.text, original.text);
        // hOCR adds a paragraph around the block's lines.
        let par = &read.blocks[0].children[0];
        assert_eq!(par.level, Some(OcrLevel::Paragraph));
        assert_eq!(par.children[0].baseline, Some(Baseline { slope: 0.0, offset: -3.0 }));
        assert_same_tree(&original, &read, false);
    }

    #[test]
    fn alto_round_trips() {
        let original = sample();
        let doc = export::alto(&original);
        let read = alto(&doc).unwrap();
        assert_eq!(read.image_size, Some((400, 300)));
        assert_eq!(read.detected_lang, Some(LangId::ENGLISH));
        assert_eq!(read.text, original.text);
        let line = &read.blocks[0].children[0];
        let b = line.baseline.unwrap();
        assert!(b.slope.abs() < 1e-6 && (b.offset + 3.0).abs() < 0.01, "{b:?}");
        // Lines without words come back with one String each. ALTO keeps
        // the outline of the rotated line, so its rotation comes back.
        let read_words: Vec<_> = read.nodes().filter(|n| n.level == Some(OcrLevel::Word)).collect();
        assert_eq!(read_words.len(), 4);
        assert_eq!(read_words[2].text, "Skewed");
        assert_eq!(read.blocks[0].children[1].angle.signum(), -1.0);
        assert!((read.blocks[0].children[1].angle + std::f32::consts::FRAC_PI_6).abs() < 0.02);
        assert_eq!(read.blocks[0].children[2].direction, Some(WritingDirection::Rtl));
        assert_eq!(read_words[1].confidence, Some(0.5));
    }

    #[test]
    fn hocr_documents_are_well_formed_and_declare_capabilities() {
        let doc = export::hocr(&sample());
        assert!(doc.starts_with("<?xml"));
        assert!(doc.contains("<meta name=\"ocr-capabilities\" content=\"ocr_page ocr_carea ocr_par ocr_line ocrx_word"));
        assert!(doc.contains("class=\"ocr_page\" id=\"page_1\" title=\"bbox 0 0 400 300; ppageno 0\""));
        assert!(doc.contains("title=\"bbox 10 20 50 32; x_wconf 96\">Hello</span>"), "{doc}");
        assert!(doc.contains("textangle 30"), "rotation is counter-clockwise degrees");
        assert!(doc.contains("dir=\"rtl\""));
        assert_balanced(&doc);
    }

    #[test]
    fn alto_documents_are_well_formed_v4() {
        let doc = export::alto(&sample());
        assert!(doc.contains("xmlns=\"http://www.loc.gov/standards/alto/ns-v4#\""));
        assert!(doc.contains("<MeasurementUnit>pixel</MeasurementUnit>"));
        assert!(doc.contains("<Page ID=\"page_1\" PHYSICAL_IMG_NR=\"1\" WIDTH=\"400\" HEIGHT=\"300\" LANG=\"en\">"));
        assert!(doc.contains(
            "<String ID=\"string_1\" HPOS=\"10\" VPOS=\"20\" WIDTH=\"40\" HEIGHT=\"12\" CONTENT=\"Hello\" WC=\"0.96\"/>"
        ));
        assert!(doc.contains("<SP HPOS=\"50\" VPOS=\"20\" WIDTH=\"10\"/>"));
        assert!(doc.contains("BASELINE=\"10,29 100,29\""), "{doc}");
        assert!(doc.contains("BASEDIRECTION=\"rtl\""));
        assert!(doc.contains("<Shape><Polygon POINTS="));
        assert_balanced(&doc);
    }

    /// Every element opened is closed in order.
    fn assert_balanced(doc: &str) {
        let mut stack = Vec::new();
        for ev in tokenize(doc) {
            match ev {
                Event::Start { name, empty: false, .. } if !VOID.contains(&name.as_str()) => stack.push(name),
                Event::End { name } => assert_eq!(stack.pop().as_deref(), Some(name.as_str()), "{doc}"),
                _ => {}
            }
        }
        assert!(stack.is_empty(), "unclosed: {stack:?}");
    }

    #[test]
    fn flat_line_results_get_the_full_hierarchy() {
        // Windows OCR's shape: lines at the top, words below, no levels.
        let r = OcrResult {
            text: "a b\nc".into(),
            blocks: vec![
                OcrBlock {
                    text: "a b".into(),
                    bounds: Some(Rect::new(0.0, 0.0, 30.0, 10.0)),
                    children: vec![
                        OcrBlock {
                            text: "a".into(),
                            bounds: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
                            ..Default::default()
                        },
                        OcrBlock {
                            text: "b".into(),
                            bounds: Some(Rect::new(20.0, 0.0, 10.0, 10.0)),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                OcrBlock { text: "c".into(), bounds: Some(Rect::new(0.0, 20.0, 10.0, 10.0)), ..Default::default() },
            ],
            ..Default::default()
        };
        let read = hocr(&export::hocr(&r)).unwrap();
        assert_eq!(read.blocks.len(), 1, "one block around the loose lines");
        assert_eq!(read.blocks[0].bounds, Some(Rect::new(0.0, 0.0, 30.0, 30.0)), "the union of its lines");
        assert_eq!(read.text, "a b\nc");
        assert_eq!(read.image_size, Some((30, 30)), "no image size: the page holds every box");
        let alto_read = alto(&export::alto(&r)).unwrap();
        assert_eq!(alto_read.text, "a b\nc");
    }

    #[test]
    fn tesseract_style_hocr_reads() {
        let doc = r#"<!DOCTYPE html><html><head><meta name='ocr-system' content='tesseract 5.3.0' /></head><body>
          <div class='ocr_page' id='page_1' title='image "a;b.png"; bbox 0 0 640 480; ppageno 0'>
           <div class='ocr_carea' id='block_1_1' title="bbox 36 92 618 361">
            <p class='ocr_par' id='par_1_1' lang='eng' title="bbox 36 92 618 184">
             <span class='ocr_line' id='line_1_1' title="bbox 36 92 580 122; baseline 0 -6; x_size 30; x_descenders 6; x_ascenders 8">
              <span class='ocrx_word' id='word_1_1' title='bbox 36 92 96 116; x_wconf 93'>The</span>
              <span class='ocrx_word' id='word_1_2' title='bbox 109 92 222 116; x_wconf 91'><strong>(quick)</strong></span>
              <span class='ocrx_word' id='word_1_3' title='bbox 236 92 282 116; x_wconf 96'>brown &amp; fox</span>
             </span>
            </p>
           </div>
          </div></body></html>"#;
        let r = hocr(doc).unwrap();
        assert_eq!(r.image_size, Some((640, 480)));
        assert_eq!(r.text, "The (quick) brown & fox");
        let words: Vec<_> = r.nodes().filter(|n| n.level == Some(OcrLevel::Word)).collect();
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].bounds, Some(Rect::new(36.0, 92.0, 60.0, 24.0)));
        assert_eq!(words[0].confidence, Some(0.93));
        assert_eq!(r.blocks[0].children[0].lang.as_deref(), Some("eng"));
    }

    #[test]
    fn old_alto_with_a_numeric_baseline_reads() {
        let doc = r#"<?xml version="1.0"?><alto xmlns="http://www.loc.gov/standards/alto/ns-v2#"><Layout>
            <Page ID="p" WIDTH="100" HEIGHT="50" PHYSICAL_IMG_NR="1"><PrintSpace>
            <ComposedBlock ID="c"><TextBlock ID="b" HPOS="1" VPOS="2" WIDTH="80" HEIGHT="20">
              <TextLine HPOS="1" VPOS="2" WIDTH="80" HEIGHT="20" BASELINE="18">
                <String CONTENT="Hi" HPOS="1" VPOS="2" WIDTH="20" HEIGHT="20" WC="0.8"/><SP/>
                <String CONTENT="there" HPOS="30" VPOS="2" WIDTH="51" HEIGHT="20"/>
              </TextLine></TextBlock></ComposedBlock></PrintSpace></Page></Layout></alto>"#;
        let r = any(doc).unwrap();
        assert_eq!(r.text, "Hi there");
        assert_eq!(r.image_size, Some((100, 50)));
        let line = &r.blocks[0].children[0];
        assert_eq!(line.baseline, Some(Baseline { slope: 0.0, offset: -4.0 }));
        assert_eq!(line.children[0].confidence, Some(0.8));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(hocr("just text").is_err());
        assert!(alto("<html/>").is_err());
        for s in
            ["", "<", "<div class='ocr_line' title='bbox 1 2'>", "<alto><TextLine BASELINE='x'>", "<String WC='nan'/>"]
        {
            let _ = any(s);
            let _ = hocr(s);
            let _ = alto(s);
        }
        let r = hocr("<span class='ocr_line' title='bbox 9 9 1 1; textangle 45'>x</span>").unwrap();
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(1.0, 1.0, 8.0, 8.0)), "45°: the box as given");
    }

    #[test]
    fn every_format_renders_an_empty_result() {
        for f in OcrFormat::ALL {
            let _ = export::render(&OcrResult::default(), f);
        }
        let empty = hocr(&export::hocr(&OcrResult::default())).unwrap();
        assert!(empty.blocks.is_empty());
        assert!(alto(&export::alto(&OcrResult::default())).unwrap().blocks.is_empty());
    }

    #[test]
    fn json_export_uses_codes_and_levels() {
        let mut r = sample();
        r.blocks[0].level = None;
        let v = export::json(&r);
        assert_eq!(v["detected_lang"], "en");
        assert_eq!(v["blocks"][0]["level"], "block");
        assert_eq!(v["blocks"][0]["children"][0]["children"][0]["confidence"], serde_json::json!(0.96f32));
        assert_eq!(v["image_size"], serde_json::json!([400, 300]));
        let back: OcrResult = serde_json::from_value(v).unwrap_or_default();
        assert!(back.blocks.is_empty() || back.blocks[0].children.len() == 3);
    }
}
