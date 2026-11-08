//! The OCR pipeline: detect lines once, straighten and orient each, then
//! read them with one or more recognisers.
//!
//! `oar-ocr`'s ready pipeline (`OAROCR`) takes exactly one recogniser. With
//! `auto` and several scripts downloaded the module reads every line with
//! each of them and keeps the most confident reading, so it drives the
//! crate's building blocks itself and runs the detector only once.
//!
//! Models load on the first request and stay until the process ends;
//! Glossa stops the process after `idle_timeout_secs`, which is what frees
//! them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbImage;
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::core::traits::adapter::{AdapterBuilder, ModelAdapter};
use oar_ocr::core::traits::task::ImageTaskInput;
use oar_ocr::core::traits::OrtConfigurable;
use oar_ocr::domain::adapters::{
    TextDetectionAdapter, TextDetectionAdapterBuilder, TextLineOrientationAdapter, TextLineOrientationAdapterBuilder,
    TextRecognitionAdapter, TextRecognitionAdapterBuilder,
};
use oar_ocr::domain::tasks::{TextDetectionConfig, TextRecognitionConfig};
use oar_ocr::oarocr::{EdgeProcessor, TextCroppingProcessor};
use oar_ocr::processors::{sort_quad_boxes, BoundingBox, LimitType};

use crate::onnx_meta;
use crate::scripts::{Script, Settings};

/// Readings below this confidence are dropped, as PaddleOCR's own pipeline
/// does (`text_rec_score_thresh`): they are mostly noise and borders.
const MIN_SCORE: f32 = 0.5;

/// Crops per recogniser run. oar-ocr's CPU default: larger batches pad
/// short lines to the longest one and gain nothing on a CPU.
const REC_BATCH: usize = 4;

/// One recognised line, in the coordinates of the picture Glossa sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    /// Four corners, clockwise from the text's top left.
    pub polygon: [[f32; 2]; 4],
    pub confidence: f32,
}

/// Where the model files are.
#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub det: PathBuf,
    pub cls: Option<PathBuf>,
}

pub struct Engine {
    loaded: Option<Loaded>,
}

struct Loaded {
    /// The settings the sessions were built with; a change rebuilds them.
    key: (usize, u32),
    det: TextDetectionAdapter,
    cls: Option<TextLineOrientationAdapter>,
    recs: HashMap<&'static str, TextRecognitionAdapter>,
}

impl Engine {
    pub fn new() -> Self {
        Self { loaded: None }
    }

    /// Recognise `image` with the given recognisers, each paired with its
    /// model file. A recogniser loads the first time it is asked for.
    pub fn recognize(
        &mut self,
        image: RgbImage,
        paths: &ModelPaths,
        scripts: &[(Script, PathBuf)],
        settings: &Settings,
    ) -> Result<Vec<Line>, String> {
        let key = (settings.threads, settings.max_side);
        if self.loaded.as_ref().is_none_or(|l| l.key != key) {
            // Free the old sessions before building new ones.
            self.loaded = None;
            self.loaded = Some(Loaded::new(key, paths, settings)?);
        }
        let loaded = self.loaded.as_mut().expect("loaded above");
        for (script, path) in scripts {
            if !loaded.recs.contains_key(script.id) {
                let rec = load_recogniser(path, settings)?;
                loaded.recs.insert(script.id, rec);
            }
        }
        let ids: Vec<&'static str> = scripts.iter().map(|(s, _)| s.id).collect();
        loaded.run(image, &ids)
    }
}

/// ONNX Runtime's own default is a thread per core, efficiency cores
/// included; on an M-series Mac that was slower than four (measured: 150 ms
/// with 4 threads, 235 ms with 8, for six lines). Four or fewer also leave
/// the rest of the machine alone.
fn threads(settings: &Settings) -> usize {
    if settings.threads > 0 {
        return settings.threads;
    }
    std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4))
}

fn ort_config(settings: &Settings) -> OrtSessionConfig {
    // Every picture and line has its own size, so the memory pattern
    // planner (which preallocates for the shapes of the previous run) only
    // adds memory: peak use fell from 260 to 200 MB without it.
    OrtSessionConfig::new().with_memory_pattern(false).with_intra_threads(threads(settings))
}

impl Loaded {
    fn new(key: (usize, u32), paths: &ModelPaths, settings: &Settings) -> Result<Self, String> {
        // PaddleOCR's own defaults for PP-OCRv5: the detector sees the
        // picture at its size (the short side only raised to 64 px), with
        // the long side capped. Raising the short side further, as RapidOCR
        // does (736), doubled the memory and split single-line captures
        // into words. The box threshold and unclip ratio are RapidOCR's:
        // with PaddleOCR's (0.6, 1.5) a long Cyrillic line came out cut.
        let det_config = TextDetectionConfig {
            score_threshold: 0.3,
            box_threshold: 0.5,
            unclip_ratio: 1.6,
            max_candidates: 1000,
            limit_side_len: Some(64),
            limit_type: Some(LimitType::Min),
            max_side_len: Some(settings.max_side),
        };
        let det = TextDetectionAdapterBuilder::new()
            .with_config(det_config)
            .with_ort_config(ort_config(settings))
            .build(paths.det.as_path())
            .map_err(|e| format!("loading the detector {}: {e}", paths.det.display()))?;
        let cls = match &paths.cls {
            Some(p) => Some(
                TextLineOrientationAdapterBuilder::new()
                    .with_ort_config(ort_config(settings))
                    .build(p.as_path())
                    .map_err(|e| format!("loading the orientation model {}: {e}", p.display()))?,
            ),
            None => None,
        };
        Ok(Self { key, det, cls, recs: HashMap::new() })
    }

    fn run(&self, image: RgbImage, recs: &[&'static str]) -> Result<Vec<Line>, String> {
        let (padded, pad) = pad(&image);
        drop(image);
        let padded = Arc::new(padded);

        let det = self
            .det
            .execute(ImageTaskInput::from_arc_images(vec![Arc::clone(&padded)]), None)
            .map_err(|e| format!("detection: {e}"))?;
        let boxes: Vec<BoundingBox> =
            det.detections.into_iter().next().unwrap_or_default().into_iter().map(|d| d.bbox).collect();
        let boxes = sort_quad_boxes(&boxes);
        if boxes.is_empty() {
            return Ok(Vec::new());
        }

        let crops = TextCroppingProcessor::new(true)
            .process((Arc::clone(&padded), boxes.clone()))
            .map_err(|e| format!("cropping lines: {e}"))?;
        drop(padded);
        let (boxes, mut crops): (Vec<BoundingBox>, Vec<Arc<RgbImage>>) =
            boxes.into_iter().zip(crops).filter_map(|(b, c)| Some((b, c?))).unzip();

        if let Some(cls) = &self.cls {
            let out = cls
                .execute(ImageTaskInput::from_arc_images(crops.clone()), None)
                .map_err(|e| format!("line orientation: {e}"))?;
            for (crop, classes) in crops.iter_mut().zip(out.classifications) {
                // Class 1 is text upside down.
                if classes.first().is_some_and(|c| c.class_id == 1) {
                    *crop = Arc::new(image::imageops::rotate180(crop.as_ref()));
                }
            }
        }

        let mut best: Vec<(String, f32)> = vec![(String::new(), 0.0); crops.len()];
        for id in recs {
            let rec = &self.recs[id];
            for (i, (text, score)) in read_lines(rec, &crops)?.into_iter().enumerate() {
                if !text.trim().is_empty() && score > best[i].1 {
                    best[i] = (text, score);
                }
            }
        }

        Ok(boxes
            .iter()
            .zip(best)
            .filter(|(_, (text, score))| *score >= MIN_SCORE && !text.trim().is_empty())
            .filter_map(|(b, (text, score))| {
                Some(Line { text: text.trim().to_owned(), polygon: corners(b, pad)?, confidence: score })
            })
            .collect())
    }
}

/// Every crop read by `rec`, in the crops' order. Crops of similar shape go
/// together, so a batch pads little.
fn read_lines(rec: &TextRecognitionAdapter, crops: &[Arc<RgbImage>]) -> Result<Vec<(String, f32)>, String> {
    let mut order: Vec<usize> = (0..crops.len()).collect();
    let ratio = |i: &usize| crops[*i].width() as f32 / crops[*i].height().max(1) as f32;
    order.sort_by(|a, b| ratio(a).total_cmp(&ratio(b)));
    let mut out = vec![(String::new(), 0.0f32); crops.len()];
    for chunk in order.chunks(REC_BATCH) {
        let input = ImageTaskInput::from_arc_images(chunk.iter().map(|&i| Arc::clone(&crops[i])).collect());
        let r = rec.execute(input, None).map_err(|e| format!("recognition: {e}"))?;
        for (k, &i) in chunk.iter().enumerate() {
            out[i] = (r.texts.get(k).cloned().unwrap_or_default(), r.scores.get(k).copied().unwrap_or(0.0));
        }
    }
    Ok(out)
}

fn load_recogniser(path: &Path, settings: &Settings) -> Result<TextRecognitionAdapter, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let dict = onnx_meta::metadata(&bytes, "character").ok_or_else(|| {
        format!("{} has no character list in its metadata; is it a RapidOCR PP-OCRv5 model?", path.display())
    })?;
    drop(bytes);
    let dict: Vec<String> = dict.lines().map(str::to_owned).collect();
    TextRecognitionAdapterBuilder::new()
        .character_dict(dict)
        .with_config(TextRecognitionConfig { score_threshold: 0.0 })
        .with_ort_config(ort_config(settings))
        .build(path)
        .map_err(|e| format!("loading the recogniser {}: {e}", path.display()))
}

/// A border around the picture: the detector misses text that touches the
/// edge, and screen captures are usually cut tight around it. The border
/// takes the colour of the top-left pixel, the likeliest background.
fn pad(image: &RgbImage) -> (RgbImage, u32) {
    let pad = (image.width().min(image.height()) / 10).clamp(8, 32);
    let colour = *image.get_pixel(0, 0);
    let mut out = RgbImage::from_pixel(image.width() + 2 * pad, image.height() + 2 * pad, colour);
    image::imageops::replace(&mut out, image, i64::from(pad), i64::from(pad));
    (out, pad)
}

fn corners(b: &BoundingBox, pad: u32) -> Option<[[f32; 2]; 4]> {
    if b.points.len() != 4 {
        return None;
    }
    let p = pad as f32;
    let mut out = [[0.0; 2]; 4];
    for (o, pt) in out.iter_mut().zip(&b.points) {
        *o = [(pt.x - p).max(0.0), (pt.y - p).max(0.0)];
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_keeps_the_picture_and_its_offset() {
        let mut img = RgbImage::from_pixel(200, 50, image::Rgb([255, 255, 255]));
        img.put_pixel(10, 20, image::Rgb([0, 0, 0]));
        let (out, pad) = pad(&img);
        assert_eq!(pad, 8);
        assert_eq!(out.dimensions(), (216, 66));
        assert_eq!(out.get_pixel(18, 28), &image::Rgb([0, 0, 0]));
        assert_eq!(out.get_pixel(0, 0), &image::Rgb([255, 255, 255]));
    }

    #[test]
    fn corners_come_back_without_the_border() {
        use oar_ocr::processors::Point;
        let b = BoundingBox::new(vec![
            Point::new(18.0, 28.0),
            Point::new(118.0, 28.0),
            Point::new(118.0, 48.0),
            Point::new(18.0, 48.0),
        ]);
        assert_eq!(corners(&b, 8), Some([[10.0, 20.0], [110.0, 20.0], [110.0, 40.0], [10.0, 40.0]]));
        assert_eq!(corners(&BoundingBox::new(vec![Point::new(1.0, 1.0)]), 8), None);
    }
}
