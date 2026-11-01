//! Writing an [`OcrResult`] out: plain text, JSON, hOCR and ALTO XML.
//!
//! Why these two XML formats: they are what other OCR software reads and
//! writes. **hOCR** (XHTML with `ocr_page`, `ocr_carea`, `ocr_par`,
//! `ocr_line` and `ocrx_word` classes, geometry in `title` properties) is
//! Tesseract's and OCRopus's output and what tools like hocr-tools, OCRmyPDF
//! and gImageReader consume. **ALTO** (Library of Congress; `Page`,
//! `PrintSpace`, `TextBlock`, `TextLine`, `String`) is what digitisation
//! projects and viewers expect. PAGE-XML is a third, mostly in layout
//! research; its model is the same tree and is not written here.
//!
//! The tree is normalised on the way out to the full page → block →
//! paragraph → line → word hierarchy both formats expect: lines found at
//! the top level (Windows OCR returns only lines) get a block and paragraph
//! around them, words without a line get one, and missing boxes are the
//! union of their children's. Per-region translations have no place in
//! either format and appear only in the JSON.

use std::fmt::Write as _;

use super::xml::escape;
use super::{OcrBlock, OcrLevel, OcrResult, Rect, WritingDirection};

/// A way to write a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OcrFormat {
    /// The recognised text.
    Text,
    /// The whole result as JSON: text, blocks with geometry, language.
    Json,
    /// hOCR 1.2 in XHTML.
    Hocr,
    /// ALTO XML version 4.
    Alto,
}

impl OcrFormat {
    pub const ALL: [OcrFormat; 4] = [OcrFormat::Text, OcrFormat::Json, OcrFormat::Hocr, OcrFormat::Alto];

    /// `text`, `json`, `hocr` or `alto` (also `txt`, `html`, `xml`).
    pub fn from_name(name: &str) -> Option<OcrFormat> {
        match name.trim().to_ascii_lowercase().as_str() {
            "text" | "txt" | "plain" => Some(OcrFormat::Text),
            "json" => Some(OcrFormat::Json),
            "hocr" | "html" | "xhtml" => Some(OcrFormat::Hocr),
            "alto" | "xml" => Some(OcrFormat::Alto),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            OcrFormat::Text => "text",
            OcrFormat::Json => "json",
            OcrFormat::Hocr => "hocr",
            OcrFormat::Alto => "alto",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            OcrFormat::Text => "text/plain; charset=utf-8",
            OcrFormat::Json => "application/json",
            OcrFormat::Hocr => "application/xhtml+xml; charset=utf-8",
            OcrFormat::Alto => "application/xml; charset=utf-8",
        }
    }

    /// Whether the format carries geometry, so the engine should be asked
    /// for words rather than just the text.
    pub fn wants_structure(self) -> bool {
        self != OcrFormat::Text
    }
}

/// `result` in `format`.
pub fn render(result: &OcrResult, format: OcrFormat) -> String {
    match format {
        OcrFormat::Text => text(result),
        OcrFormat::Json => serde_json::to_string_pretty(&json(result)).unwrap_or_default(),
        OcrFormat::Hocr => hocr(result),
        OcrFormat::Alto => alto(result),
    }
}

/// The recognised text, from the tree when the result carries no text.
pub fn text(result: &OcrResult) -> String {
    if result.text.trim().is_empty() {
        result.text_from_blocks()
    } else {
        result.text.clone()
    }
}

/// The result as JSON, for people and programs rather than for Glossa's own
/// round trips: levels filled in and the detected language as a code.
pub fn json(result: &OcrResult) -> serde_json::Value {
    let mut r = result.clone().with_levels();
    if r.text.trim().is_empty() {
        r.text = r.text_from_blocks();
    }
    let lang = r.detected_lang.filter(|l| l.is_language()).map(|l| l.bcp47());
    // Through text rather than `to_value`, so `f32`s print as written
    // (0.97, not 0.9700000286102295).
    let mut v: serde_json::Value =
        serde_json::to_string(&r).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if let Some(obj) = v.as_object_mut() {
        match lang {
            Some(code) => obj.insert("detected_lang".into(), code.into()),
            None => obj.remove("detected_lang"),
        };
        if r.provider_id.is_empty() {
            obj.remove("provider_id");
        }
    }
    v
}

// -- the normalised tree -------------------------------------------------------

struct Block<'a> {
    node: Option<&'a OcrBlock>,
    pars: Vec<Par<'a>>,
}

struct Par<'a> {
    node: Option<&'a OcrBlock>,
    lines: Vec<Line<'a>>,
}

struct Line<'a> {
    node: Option<&'a OcrBlock>,
    words: Vec<&'a OcrBlock>,
}

fn level(n: &OcrBlock) -> OcrLevel {
    n.level.unwrap_or(OcrLevel::Line)
}

fn normalise(blocks: &[OcrBlock]) -> Vec<Block<'_>> {
    let mut out = Vec::new();
    let mut loose: Vec<&OcrBlock> = Vec::new();
    for b in blocks {
        if level(b) == OcrLevel::Block {
            if !loose.is_empty() {
                out.push(Block { node: None, pars: pars_of(&std::mem::take(&mut loose)) });
            }
            out.push(Block { node: Some(b), pars: pars_of(&b.children.iter().collect::<Vec<_>>()) });
        } else {
            loose.push(b);
        }
    }
    if !loose.is_empty() {
        out.push(Block { node: None, pars: pars_of(&loose) });
    }
    out
}

fn pars_of<'a>(nodes: &[&'a OcrBlock]) -> Vec<Par<'a>> {
    let mut out = Vec::new();
    let mut loose: Vec<&OcrBlock> = Vec::new();
    for &n in nodes {
        if matches!(level(n), OcrLevel::Paragraph | OcrLevel::Block) {
            if !loose.is_empty() {
                out.push(Par { node: None, lines: lines_of(&std::mem::take(&mut loose)) });
            }
            out.push(Par { node: Some(n), lines: lines_of(&n.children.iter().collect::<Vec<_>>()) });
        } else {
            loose.push(n);
        }
    }
    if !loose.is_empty() {
        out.push(Par { node: None, lines: lines_of(&loose) });
    }
    out
}

fn lines_of<'a>(nodes: &[&'a OcrBlock]) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    let mut loose: Vec<&OcrBlock> = Vec::new();
    for &n in nodes {
        if level(n) == OcrLevel::Word {
            loose.push(n);
            continue;
        }
        if !loose.is_empty() {
            out.push(Line { node: None, words: std::mem::take(&mut loose) });
        }
        // A line, or something deeper nested where a line belongs: its
        // words are whatever leaves it has.
        let mut words = Vec::new();
        collect_words(n, &mut words);
        out.push(Line { node: Some(n), words });
    }
    if !loose.is_empty() {
        out.push(Line { node: None, words: loose });
    }
    out
}

fn collect_words<'a>(n: &'a OcrBlock, out: &mut Vec<&'a OcrBlock>) {
    for c in &n.children {
        if c.children.is_empty() {
            out.push(c);
        } else {
            collect_words(c, out);
        }
    }
}

fn union(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    let mut it = rects.into_iter().filter(|r| r.width.is_finite() && r.height.is_finite());
    let first = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.width, first.y + first.height);
    for r in it {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.width);
        y1 = y1.max(r.y + r.height);
    }
    Some(Rect::from_corners(x0, y0, x1, y1))
}

/// A node's axis-aligned box, or its children's union.
fn node_box(n: &OcrBlock) -> Option<Rect> {
    n.bbox().or_else(|| union(n.children.iter().filter_map(node_box)))
}

impl Line<'_> {
    fn bbox(&self) -> Option<Rect> {
        self.node.and_then(node_box).or_else(|| union(self.words.iter().filter_map(|w| node_box(w))))
    }

    fn text(&self) -> String {
        match self.node {
            Some(n) if !n.text.trim().is_empty() => n.text.trim().to_owned(),
            _ => self.words.iter().map(|w| w.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" "),
        }
    }
}

impl Par<'_> {
    fn bbox(&self) -> Option<Rect> {
        self.node.and_then(node_box).or_else(|| union(self.lines.iter().filter_map(Line::bbox)))
    }
}

impl Block<'_> {
    fn bbox(&self) -> Option<Rect> {
        self.node.and_then(node_box).or_else(|| union(self.pars.iter().filter_map(Par::bbox)))
    }
}

/// The page size: the image's when known, else enough to hold every box.
fn page_size(result: &OcrResult, blocks: &[Block]) -> (i64, i64) {
    if let Some((w, h)) = result.image_size {
        return (w as i64, h as i64);
    }
    union(blocks.iter().filter_map(Block::bbox))
        .map(|r| (px(r.x + r.width).max(0), px(r.y + r.height).max(0)))
        .unwrap_or((0, 0))
}

fn px(v: f32) -> i64 {
    if v.is_finite() {
        v.round() as i64
    } else {
        0
    }
}

/// `x0 y0 x1 y1`, clamped at zero.
fn corners(r: Rect) -> (i64, i64, i64, i64) {
    let (x0, y0) = (px(r.x).max(0), px(r.y).max(0));
    (x0, y0, px(r.x + r.width).max(x0), px(r.y + r.height).max(y0))
}

/// Counter-clockwise degrees, as both formats measure rotation, from our
/// clockwise radians. `None` for level text.
fn ccw_degrees(n: Option<&OcrBlock>) -> Option<f32> {
    let a = n?.angle;
    if a == 0.0 || !a.is_finite() {
        return None;
    }
    let d = (-a.to_degrees() * 100.0).round() / 100.0;
    (d != 0.0).then_some(d)
}

/// The polygon worth writing: the producer's own, else the corners of
/// rotated text. Level text has none.
fn outline(n: Option<&OcrBlock>) -> Vec<[f32; 2]> {
    let Some(n) = n else { return Vec::new() };
    if n.polygon.len() >= 3 {
        return n.polygon.clone();
    }
    match (n.bounds, n.angle) {
        (Some(r), a) if a != 0.0 && a.is_finite() => r.rotated_corners(a).to_vec(),
        _ => Vec::new(),
    }
}

fn page_lang(result: &OcrResult) -> Option<&'static str> {
    result.detected_lang.filter(|l| l.is_language()).map(|l| l.bcp47())
}

// -- hOCR ---------------------------------------------------------------------------

/// The result as an hOCR 1.2 document (XHTML 1.0, one `ocr_page`). Boxes are
/// axis-aligned (`bbox`); rotated text adds `textangle` and a `poly`
/// outline; confidence is `x_wconf` (0–100) on words and wherever the
/// engine gave one.
pub fn hocr(result: &OcrResult) -> String {
    let r = result.clone().with_levels();
    let blocks = normalise(&r.blocks);
    let (w, h) = page_size(&r, &blocks);
    let lang = page_lang(&r);
    let mut out = String::with_capacity(1024 + r.text.len() * 8);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \
         \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">\n",
    );
    let _ = writeln!(
        out,
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"{l}\" lang=\"{l}\">",
        l = lang.unwrap_or("und")
    );
    out.push_str(" <head>\n  <title></title>\n");
    out.push_str("  <meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\"/>\n");
    let system = if r.provider_id.is_empty() {
        format!("glossa {}", crate::VERSION)
    } else {
        format!("glossa {} ({})", crate::VERSION, r.provider_id)
    };
    let _ = writeln!(out, "  <meta name=\"ocr-system\" content=\"{}\"/>", escape(&system));
    out.push_str("  <meta name=\"ocr-number-of-pages\" content=\"1\"/>\n");
    let langs = lang.map(|l| l.split('-').next().unwrap_or(l)).unwrap_or("unknown");
    let _ = writeln!(out, "  <meta name=\"ocr-langs\" content=\"{}\"/>", escape(langs));
    out.push_str("  <meta name=\"ocr-scripts\" content=\"unknown\"/>\n");
    out.push_str(
        "  <meta name=\"ocr-capabilities\" content=\"ocr_page ocr_carea ocr_par ocr_line ocrx_word \
         ocrp_wconf ocrp_lang ocrp_dir ocrp_poly\"/>\n",
    );
    out.push_str(" </head>\n <body>\n");
    let _ = writeln!(out, "  <div class=\"ocr_page\" id=\"page_1\" title=\"bbox 0 0 {w} {h}; ppageno 0\">");
    let (mut nb, mut np, mut nl, mut nw) = (0, 0, 0, 0);
    for b in &blocks {
        nb += 1;
        let _ =
            writeln!(out, "   <div class=\"ocr_carea\" id=\"block_1_{nb}\"{}>", hocr_attrs(b.node, b.bbox(), false));
        for p in &b.pars {
            np += 1;
            let _ = writeln!(out, "    <p class=\"ocr_par\" id=\"par_1_{np}\"{}>", hocr_attrs(p.node, p.bbox(), false));
            for l in &p.lines {
                nl += 1;
                let _ = write!(
                    out,
                    "     <span class=\"ocr_line\" id=\"line_1_{nl}\"{}>",
                    hocr_attrs(l.node, l.bbox(), true)
                );
                if l.words.is_empty() {
                    out.push_str(&escape(&l.text()));
                } else {
                    for (i, word) in l.words.iter().enumerate() {
                        nw += 1;
                        if i > 0 {
                            out.push(' ');
                        }
                        let _ = write!(
                            out,
                            "<span class=\"ocrx_word\" id=\"word_1_{nw}\"{}>{}</span>",
                            hocr_attrs(Some(word), node_box(word), false),
                            escape(word.text.trim())
                        );
                    }
                }
                out.push_str("</span>\n");
            }
            out.push_str("    </p>\n");
        }
        out.push_str("   </div>\n");
    }
    out.push_str("  </div>\n </body>\n</html>\n");
    out
}

/// ` title="…" lang="…" dir="…"` for one element.
fn hocr_attrs(node: Option<&OcrBlock>, bbox: Option<Rect>, is_line: bool) -> String {
    let mut props: Vec<String> = Vec::new();
    if let Some(r) = bbox {
        let (x0, y0, x1, y1) = corners(r);
        props.push(format!("bbox {x0} {y0} {x1} {y1}"));
    }
    if let Some(n) = node {
        if let (true, Some(b)) = (is_line, n.baseline) {
            props.push(format!("baseline {} {}", round3(b.slope), px(b.offset)));
        }
        if let Some(d) = ccw_degrees(Some(n)) {
            props.push(format!("textangle {d}"));
        }
        let poly = outline(Some(n));
        if !poly.is_empty() {
            let pts: Vec<String> = poly.iter().map(|p| format!("{} {}", px(p[0]).max(0), px(p[1]).max(0))).collect();
            props.push(format!("poly {}", pts.join(" ")));
        }
        if let Some(c) = n.confidence.filter(|c| c.is_finite()) {
            props.push(format!("x_wconf {}", (c.clamp(0.0, 1.0) * 100.0).round() as i64));
        }
    }
    let mut out = String::new();
    if !props.is_empty() {
        let _ = write!(out, " title=\"{}\"", escape(&props.join("; ")));
    }
    if let Some(n) = node {
        if let Some(l) = n.lang.as_deref().filter(|l| !l.is_empty()) {
            let _ = write!(out, " lang=\"{}\"", escape(l));
        }
        match n.direction {
            Some(WritingDirection::Rtl) => out.push_str(" dir=\"rtl\""),
            Some(WritingDirection::Ltr) => out.push_str(" dir=\"ltr\""),
            Some(WritingDirection::Ttb) => out.push_str(" style=\"writing-mode:vertical-rl\""),
            None => {}
        }
    }
    out
}

fn round3(v: f32) -> f32 {
    if v.is_finite() {
        (v * 1000.0).round() / 1000.0
    } else {
        0.0
    }
}

// -- ALTO ---------------------------------------------------------------------------

/// The result as an ALTO version 4 document (schema 4.4, measurement unit
/// pixel). ALTO has no paragraphs, so a paragraph's lines sit directly in
/// their `TextBlock`. Word confidence is `WC` (0–1); a rotated block carries
/// `ROTATION` (degrees, counter-clockwise) and any rotated element its
/// outline as a `Shape/Polygon`.
pub fn alto(result: &OcrResult) -> String {
    let r = result.clone().with_levels();
    let blocks = normalise(&r.blocks);
    let (w, h) = page_size(&r, &blocks);
    let mut out = String::with_capacity(1024 + r.text.len() * 8);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<alto xmlns=\"http://www.loc.gov/standards/alto/ns-v4#\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:schemaLocation=\"http://www.loc.gov/standards/alto/ns-v4# \
         http://www.loc.gov/standards/alto/v4/alto-4-4.xsd\" SCHEMAVERSION=\"4.4\">\n",
    );
    out.push_str("  <Description>\n    <MeasurementUnit>pixel</MeasurementUnit>\n");
    out.push_str("    <Processing ID=\"processing_1\">\n");
    if !r.provider_id.is_empty() {
        let _ =
            writeln!(out, "      <processingStepSettings>engine={}</processingStepSettings>", escape(&r.provider_id));
    }
    let _ = writeln!(
        out,
        "      <processingSoftware>\n        <softwareName>Glossa</softwareName>\n        \
         <softwareVersion>{}</softwareVersion>\n      </processingSoftware>",
        crate::VERSION
    );
    out.push_str("    </Processing>\n  </Description>\n  <Layout>\n");
    let lang = page_lang(&r).map(|l| format!(" LANG=\"{}\"", escape(l))).unwrap_or_default();
    let _ = writeln!(out, "    <Page ID=\"page_1\" PHYSICAL_IMG_NR=\"1\" WIDTH=\"{w}\" HEIGHT=\"{h}\"{lang}>");
    let _ = writeln!(out, "      <PrintSpace HPOS=\"0\" VPOS=\"0\" WIDTH=\"{w}\" HEIGHT=\"{h}\">");
    let (mut nb, mut nl, mut ns) = (0, 0, 0);
    for b in &blocks {
        nb += 1;
        let mut attrs = alto_box(b.bbox());
        if let Some(d) = ccw_degrees(b.node) {
            let _ = write!(attrs, " ROTATION=\"{d}\"");
        }
        attrs.push_str(&alto_lang_dir(b.node));
        let _ = writeln!(out, "        <TextBlock ID=\"block_{nb}\"{attrs}>");
        write_shape(&mut out, b.node, "          ");
        for p in &b.pars {
            for l in &p.lines {
                let text = l.text();
                if text.is_empty() {
                    continue;
                }
                nl += 1;
                let bbox = l.bbox();
                let mut attrs = alto_box(bbox);
                if let (Some(n), Some(r)) = (l.node, bbox) {
                    if let Some(b) = n.baseline {
                        let (x0, _, x1, y1) = corners(r);
                        let y_start = y1 as f32 + b.offset;
                        let y_end = y_start + b.slope * (x1 - x0) as f32;
                        let _ = write!(attrs, " BASELINE=\"{x0},{} {x1},{}\"", px(y_start).max(0), px(y_end).max(0));
                    }
                }
                attrs.push_str(&alto_lang_dir(l.node));
                let _ = writeln!(out, "          <TextLine ID=\"line_{nl}\"{attrs}>");
                write_shape(&mut out, l.node, "            ");
                if l.words.is_empty() {
                    ns += 1;
                    let wc = l.node.and_then(|n| n.confidence).map(wc_attr).unwrap_or_default();
                    let _ = writeln!(
                        out,
                        "            <String ID=\"string_{ns}\"{} CONTENT=\"{}\"{wc}/>",
                        alto_box(bbox),
                        escape(&text)
                    );
                } else {
                    // The right edge of the previous word, once there is one.
                    let mut prev_end: Option<Option<i64>> = None;
                    for word in l.words.iter().filter(|w| !w.text.trim().is_empty()) {
                        let wbox = node_box(word);
                        match (prev_end, wbox) {
                            (Some(Some(end)), Some(r)) => {
                                let (x0, y0, _, _) = corners(r);
                                let gap = (x0 - end).max(0);
                                let _ = writeln!(out, "            <SP HPOS=\"{end}\" VPOS=\"{y0}\" WIDTH=\"{gap}\"/>");
                            }
                            (Some(_), _) => out.push_str("            <SP/>\n"),
                            (None, _) => {}
                        }
                        ns += 1;
                        let wc = word.confidence.map(wc_attr).unwrap_or_default();
                        let lang = word
                            .lang
                            .as_deref()
                            .filter(|l| !l.is_empty())
                            .map(|l| format!(" LANG=\"{}\"", escape(l)))
                            .unwrap_or_default();
                        let shape = outline(Some(word));
                        let open = format!(
                            "            <String ID=\"string_{ns}\"{} CONTENT=\"{}\"{wc}{lang}",
                            alto_box(wbox),
                            escape(word.text.trim())
                        );
                        if shape.is_empty() {
                            let _ = writeln!(out, "{open}/>");
                        } else {
                            let _ = writeln!(out, "{open}>");
                            write_shape(&mut out, Some(word), "              ");
                            out.push_str("            </String>\n");
                        }
                        prev_end = Some(wbox.map(|r| corners(r).2));
                    }
                }
                out.push_str("          </TextLine>\n");
            }
        }
        out.push_str("        </TextBlock>\n");
    }
    out.push_str("      </PrintSpace>\n    </Page>\n  </Layout>\n</alto>\n");
    out
}

fn alto_box(r: Option<Rect>) -> String {
    match r {
        Some(r) => {
            let (x0, y0, x1, y1) = corners(r);
            format!(" HPOS=\"{x0}\" VPOS=\"{y0}\" WIDTH=\"{}\" HEIGHT=\"{}\"", x1 - x0, y1 - y0)
        }
        None => String::new(),
    }
}

fn alto_lang_dir(n: Option<&OcrBlock>) -> String {
    let Some(n) = n else { return String::new() };
    let mut out = String::new();
    if let Some(l) = n.lang.as_deref().filter(|l| !l.is_empty()) {
        let _ = write!(out, " LANG=\"{}\"", escape(l));
    }
    if let Some(d) = n.direction {
        let d = match d {
            WritingDirection::Ltr => "ltr",
            WritingDirection::Rtl => "rtl",
            WritingDirection::Ttb => "ttb",
        };
        let _ = write!(out, " BASEDIRECTION=\"{d}\"");
    }
    out
}

fn wc_attr(c: f32) -> String {
    if !c.is_finite() {
        return String::new();
    }
    format!(" WC=\"{}\"", (c.clamp(0.0, 1.0) * 1000.0).round() / 1000.0)
}

fn write_shape(out: &mut String, n: Option<&OcrBlock>, indent: &str) {
    let poly = outline(n);
    if poly.is_empty() {
        return;
    }
    let pts: Vec<String> = poly.iter().map(|p| format!("{},{}", px(p[0]).max(0), px(p[1]).max(0))).collect();
    let _ = writeln!(out, "{indent}<Shape><Polygon POINTS=\"{}\"/></Shape>", pts.join(" "));
}
