mod db;
mod execution_engine;
mod profile_store;
mod undo_engine;

use anyhow::Result;
use chrono::{Datelike, NaiveDateTime, Utc};
use db::{CachedPhotoData, Database};
use eframe::egui;
use execution_engine::{ExecutionEngine, RawPhotoInput, TransferMode};
use image::DynamicImage;
use kamadak_exif::{In, Reader, Tag, Value};
use ort::session::Session;
use ort::value::Tensor;
use profile_store::ProfileStore;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

fn main() -> eframe::Result<()> {
    let _ = ort::init().commit();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 840.0])
            .with_title("Photo Organizer - Integrated Suite"),
        ..Default::default()
    };
    eframe::run_native(
        "Photo Organizer",
        options,
        Box::new(|_cc| Box::new(PhotoOrganizerApp::new())),
    )
}

struct ProcessedPayload {
    source_path: PathBuf,
    year: u32,
    month: u32,
    is_exif: bool,
    category: String,
    confidence: f32,
    image: egui::ColorImage,
}

pub struct StagedItem {
    pub source_path: PathBuf,
    pub year: u32,
    pub month: u32,
    pub is_exif: bool,
    pub category: String,
    pub confidence: f32,
    pub texture: egui::TextureHandle,
    pub selected: bool,
}

pub struct PhotoOrganizerApp {
    input_folder: Option<PathBuf>,
    output_folder: Option<PathBuf>,
    items: Vec<StagedItem>,
    is_processing: bool,
    profiles: ProfileStore,
    tx: Sender<ProcessedPayload>,
    rx: Receiver<ProcessedPayload>,
}

impl PhotoOrganizerApp {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            input_folder: None,
            output_folder: None,
            items: Vec::new(),
            is_processing: false,
            profiles: ProfileStore::default(),
            tx,
            rx,
        }
    }

    fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.is_processing = true;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();

        thread::spawn(move || {
            let db = Database::init("photo_cache.db").ok();
            let mut session = Session::builder()
                .and_then(|mut b| b.commit_from_file("models/clip_visual.onnx"))
                .ok();

            if let Ok(entries) = fs::read_dir(&folder) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }

                    let mut cached = None;
                    let file_hash = Database::compute_file_hash(&path).unwrap_or_default();

                    if let Some(ref db_conn) = db {
                        if let Ok(Some(data)) = db_conn.get_cached(&file_hash) {
                            cached = Some(data);
                        }
                    }

                    let (year, month, is_exif, embedding) = match cached {
                        Some(c) => (c.year, c.month, c.is_exif_date, c.embedding),
                        None => {
                            let date_info = extract_date(&path);
                            let emb = if let (Some(ref mut sess), Ok(dyn_img)) =
                                (&mut session, load_image(&path))
                            {
                                extract_embedding(sess, &dyn_img).unwrap_or_default()
                            } else {
                                Vec::new()
                            };

                            if let Some(ref db_conn) = db {
                                let _ = db_conn.insert_cache(
                                    &file_hash,
                                    &CachedPhotoData {
                                        year: date_info.0,
                                        month: date_info.1,
                                        is_exif_date: date_info.2,
                                        embedding: emb.clone(),
                                    },
                                );
                            }
                            (date_info.0, date_info.1, date_info.2, emb)
                        }
                    };

                    if let Ok(dyn_img) = load_image(&path) {
                        let (category, confidence) = profiles.classify(&embedding);
                        let image = dynamic_to_egui(&dyn_img);

                        let _ = tx.send(ProcessedPayload {
                            source_path: path,
                            year,
                            month,
                            is_exif,
                            category,
                            confidence,
                            image,
                        });
                        ctx.request_repaint();
                    }
                }
            }
        });
    }

    fn execute_transfer(&mut self, mode: TransferMode) {
        let out_dir = match &self.output_folder {
            Some(p) => p.clone(),
            None => return,
        };

        let inputs: Vec<RawPhotoInput> = self
            .items
            .iter()
            .filter(|i| i.selected)
            .map(|i| RawPhotoInput {
                source_path: i.source_path.clone(),
                subject: i.category.clone(),
                year: i.year,
                month: i.month,
            })
            .collect();

        let engine = ExecutionEngine::new(out_dir, mode);
        let plan = engine.plan_batch(&inputs);
        let manifest = engine.execute_batch(&plan, |curr, total, _| {
            println!("Executing {}/{}", curr, total);
        });

        let _ = fs::write(
            "last_execution_manifest.json",
            serde_json::to_string_pretty(&manifest).unwrap(),
        );
        self.items.retain(|i| !i.selected);
    }
}

impl eframe::App for PhotoOrganizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(payload) = self.rx.try_recv() {
            let filename = payload
                .source_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let texture = ctx.load_texture(filename, payload.image, egui::TextureOptions::LINEAR);

            self.items.push(StagedItem {
                source_path: payload.source_path,
                year: payload.year,
                month: payload.month,
                is_exif: payload.is_exif,
                category: payload.category,
                confidence: payload.confidence,
                texture,
                selected: true,
            });
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Select Source Folder").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.input_folder = Some(p.clone());
                        self.start_scan(ctx.clone(), p);
                    }
                }
                if ui.button("Select Output Folder").clicked() {
                    self.output_folder = rfd::FileDialog::new().pick_folder();
                }

                ui.separator();
                if ui.button("Execute Move").clicked() {
                    self.execute_transfer(TransferMode::Move);
                }
                if ui.button("Execute Copy").clicked() {
                    self.execute_transfer(TransferMode::Copy);
                }
                if ui.button("Undo Last Run").clicked() {
                    let _ = undo_engine::UndoEngine::rollback_from_file(
                        "last_execution_manifest.json",
                        |_, _, _| {},
                    );
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("grid")
                    .num_columns(4)
                    .spacing([12.0, 12.0])
                    .show(ui, |ui| {
                        for (idx, item) in self.items.iter_mut().enumerate() {
                            ui.vertical(|ui| {
                                ui.image(&item.texture);
                                ui.checkbox(
                                    &mut item.selected,
                                    item.source_path.file_name().unwrap().to_str().unwrap(),
                                );

                                ui.horizontal(|ui| {
                                    ui.label(format!("{}/{:02}", item.year, item.month));
                                    ui.text_edit_singleline(&mut item.category);
                                });
                            });
                            if (idx + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });
            });
        });
    }
}

// Helpers
fn extract_date(path: &Path) -> (u32, u32, bool) {
    if let Ok(file) = File::open(path) {
        let mut buf = BufReader::new(file);
        if let Ok(exif_data) = Reader::new().read_from_container(&mut buf) {
            if let Some(field) = exif_data.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
                if let Value::Ascii(ref v) = field.value {
                    if let Some(bytes) = v.first() {
                        if let Ok(s) = std::str::from_utf8(bytes) {
                            if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S") {
                                return (dt.year() as u32, dt.month(), true);
                            }
                        }
                    }
                }
            }
        }
    }
    let dt: chrono::DateTime<Utc> = fs::metadata(path)
        .and_then(|m| m.created())
        .unwrap_or(std::time::SystemTime::now())
        .into();
    (dt.year() as u32, dt.month(), false)
}

fn load_image(path: &Path) -> Result<DynamicImage> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let raw_extensions = ["cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "pef"];

    if raw_extensions.contains(&ext.as_str()) {
        let raw = rawloader::decode_file(path)
            .map_err(|e| anyhow::anyhow!("RAW decoding failed: {:?}", e))?;

        // Extract preview image data from raw image metadata
        let width = raw.width;
        let height = raw.height;
        let mut image_buf = image::RgbImage::new(width as u32, height as u32);

        if let rawloader::RawImageData::Integer(data) = raw.data {
            for (idx, pixel) in image_buf.pixels_mut().enumerate() {
                if idx < data.len() {
                    let val = (data[idx] >> 6) as u8; // Scale 14-bit raw down to 8-bit
                    *pixel = image::Rgb([val, val, val]);
                }
            }
            return Ok(DynamicImage::ImageRgb8(image_buf));
        }
        Err(anyhow::anyhow!("Could not decode RAW frame"))
    } else {
        Ok(image::open(path)?)
    }
}

fn extract_embedding(session: &mut Session, img: &DynamicImage) -> Result<Vec<f32>> {
    let resized = img
        .resize_exact(224, 224, image::imageops::FilterType::Triangle)
        .to_rgb8();

    let mut data = Vec::with_capacity(1 * 3 * 224 * 224);
    // Dynamic image is RGB row-major; convert to NCHW format
    for channel in 0..3 {
        for y in 0..224 {
            for x in 0..224 {
                let pixel = resized.get_pixel(x, y);
                let val = match channel {
                    0 => (pixel[0] as f32 / 255.0 - 0.485) / 0.229,
                    1 => (pixel[1] as f32 / 255.0 - 0.456) / 0.224,
                    _ => (pixel[2] as f32 / 255.0 - 0.406) / 0.225,
                };
                data.push(val);
            }
        }
    }

    // Using shape tuple directly avoids ndarray trait version mismatches
    let input_tensor = Tensor::from_array((vec![1usize, 3, 224, 224], data))?;
    let outputs = session.run(ort::inputs!["input" => input_tensor])?;

    // try_extract_tensor returns `(&Shape, &[f32])`. Access raw slice via `.1`
    let output_ref = outputs.get("output").unwrap().try_extract_tensor::<f32>()?;
    let slice = output_ref.1;

    let norm: f32 = slice.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        Ok(slice.to_vec())
    } else {
        Ok(slice.iter().map(|x| x / norm).collect())
    }
}

fn dynamic_to_egui(img: &DynamicImage) -> egui::ColorImage {
    let thumb = img.thumbnail(200, 140).to_rgba8();
    let size = [thumb.width() as usize, thumb.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, thumb.as_raw())
}
