//! The inspection modal: a large view of one photo with keyboard navigation
//! and the category controls.

use super::chrome;
use super::layout;
use super::PhotoOrganizerApp;
use crate::app::models::ModalPreview;
use eframe::egui;

impl PhotoOrganizerApp {
    /// Opens the modal on `index` and starts loading the full-resolution image
    /// on a background thread. Loading updates arrive via `drain_high_res`.
    pub fn open_modal(&mut self, index: usize, ctx: &egui::Context) {
        if index >= self.items.len() {
            return;
        }
        let path = self.items[index].source_path.clone();

        self.modal_preview = Some(ModalPreview {
            item_index: index,
            high_res_texture: None,
            high_res_path: None,
            is_loading: true,
        });

        let tx = self.high_res_tx.clone();
        let ctx_clone = ctx.clone();
        std::thread::spawn(move || {
            if let Ok(dyn_img) = crate::media::load_image(&path) {
                let color_img = crate::media::dynamic_to_preview_color_image(&dyn_img, 1920);
                let _ = tx.send((path, color_img));
                ctx_clone.request_repaint();
            }
        });
    }

    /// Moves the modal `delta` items, wrapping around both ends.
    pub fn navigate_modal(&mut self, ctx: &egui::Context, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        if let Some(modal) = &self.modal_preview {
            let current = modal.item_index as isize;
            let len = self.items.len() as isize;
            let new_index = ((current + delta).rem_euclid(len)) as usize;
            self.open_modal(new_index, ctx);
        }
    }

    /// Uploads any full-resolution image a background thread has finished
    /// loading, ignoring results for a photo the modal has moved away from.
    pub(super) fn drain_high_res(&mut self, ctx: &egui::Context) {
        while let Ok((path, color_img)) = self.high_res_rx.try_recv() {
            let Some(modal) = &mut self.modal_preview else {
                continue;
            };
            let still_showing = self
                .items
                .get(modal.item_index)
                .is_some_and(|item| item.source_path == path);
            if !still_showing {
                continue;
            }

            let filename = path.file_name().unwrap_or_default().to_string_lossy();
            let texture = ctx.load_texture(
                format!("modal_{filename}"),
                color_img,
                egui::TextureOptions::LINEAR,
            );
            modal.high_res_texture = Some(texture);
            modal.high_res_path = Some(path);
            modal.is_loading = false;
        }
    }

    /// Draws the modal over the rest of the UI, if it is open.
    ///
    /// Keyboard and button presses are collected first and acted on after the
    /// frame, since acting on them mid-draw would re-enter `open_modal`.
    pub(super) fn render_modal(&mut self, ctx: &egui::Context) {
        let modal_index = match &self.modal_preview {
            Some(m) => m.item_index,
            None => return,
        };

        if modal_index >= self.items.len() {
            self.modal_preview = None;
            return;
        }

        let keys = modal_key_presses(ctx);

        if keys.escape {
            self.modal_preview = None;
            return;
        }
        if keys.prev {
            self.navigate_modal(ctx, -1);
            return;
        }
        if keys.next {
            self.navigate_modal(ctx, 1);
            return;
        }

        if keys.toggle_select {
            if let Some(item) = self.items.get_mut(modal_index) {
                item.selected = !item.selected;
            }
        }

        let screen_rect = ctx.screen_rect();
        let item_count = self.items.len();
        let is_loading = self.modal_preview.as_ref().is_some_and(|m| m.is_loading);
        let high_res_tex = self
            .modal_preview
            .as_ref()
            .and_then(|m| m.high_res_texture.clone());

        let mut close_modal = false;
        let mut prev_requested = false;
        let mut next_requested = false;
        let mut single_train_requested = false;

        let item = &mut self.items[modal_index];
        let filename = item
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let modal_size = layout::inspection_modal_size(screen_rect.size());

        // Split out so the closure below can borrow the item and the profiles
        // separately: `render_modal_controls` needs both.
        let profiles = &self.profiles;
        let item = &mut self.items[modal_index];

        let (_, frame) =
            chrome::show_modal_card(ctx, "photo_verification_modal_area", modal_size, |ui| {
                // Header / Title & Metadata
                ui.horizontal(|ui| {
                    ui.checkbox(&mut item.selected, "☑ Selected");
                    ui.separator();
                    ui.label(egui::RichText::new(&filename).strong());
                    ui.separator();
                    ui.label(format!("Date: {}/{:02}", item.year, item.month));
                    if is_loading {
                        ui.separator();
                        ui.spinner();
                        ui.colored_label(egui::Color32::LIGHT_GRAY, "Loading full image...");
                    } else if high_res_tex.is_some() {
                        ui.separator();
                        ui.colored_label(egui::Color32::from_rgb(0, 200, 100), "✨ High-Res");
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✖").clicked() {
                            close_modal = true;
                        }
                    });
                });
                ui.separator();

                // Image View
                let active_tex = high_res_tex.as_ref().unwrap_or(&item.texture);
                let tex_size = active_tex.size_vec2();
                let avail_w = ui.available_width();
                let avail_h = (ui.available_height() - 50.0).max(150.0);

                let img_aspect = tex_size.x / tex_size.y;
                let container_aspect = avail_w / avail_h;

                let (disp_w, disp_h) = if img_aspect > container_aspect {
                    (avail_w, avail_w / img_aspect)
                } else {
                    (avail_h * img_aspect, avail_h)
                };

                ui.vertical_centered(|ui| {
                    ui.image(egui::load::SizedTexture::new(
                        active_tex.id(),
                        [disp_w, disp_h],
                    ));
                });

                ui.separator();

                // Bottom Navigation & Controls. The custom category input is
                // inline here, unlike the grid cell.
                let actions =
                    Self::render_modal_controls(profiles, ui, item, modal_index, item_count);
                prev_requested = actions.prev;
                next_requested = actions.next;
                single_train_requested = actions.train;
                close_modal |= actions.close;
            });

        close_modal |= frame.close;

        if close_modal {
            self.modal_preview = None;
        } else if prev_requested {
            self.navigate_modal(ctx, -1);
        } else if next_requested {
            self.navigate_modal(ctx, 1);
        } else if single_train_requested {
            let cat = self.items[modal_index].category.clone();
            self.train_single_item(modal_index, &cat);
        }
    }
}

/// The modal keys pressed this frame.
struct ModalKeys {
    escape: bool,
    prev: bool,
    next: bool,
    toggle_select: bool,
}

fn modal_key_presses(ctx: &egui::Context) -> ModalKeys {
    ctx.input(|i| ModalKeys {
        escape: i.key_pressed(egui::Key::Escape),
        prev: i.key_pressed(egui::Key::ArrowLeft),
        next: i.key_pressed(egui::Key::ArrowRight),
        toggle_select: i.key_pressed(egui::Key::Space),
    })
}
