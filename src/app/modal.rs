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

    /// Moves the modal `delta` photos, wrapping around both ends.
    ///
    /// Steps through the photos in view, not through `self.items`: the modal is
    /// inspecting the grid the user is looking at, so arrowing past the edge of a
    /// filtered grid has to land on nothing rather than on a photo the filter
    /// excluded. Paging around the whole folder would also make the position
    /// counter jump, since "3 / 40" would no longer mean three of forty.
    pub fn navigate_modal(&mut self, ctx: &egui::Context, delta: isize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let Some(current) = self.modal_preview.as_ref().map(|m| m.item_index) else {
            return;
        };
        // The photo on show is no longer in view: a filter moved underneath an
        // open modal. Left alone rather than jumped somewhere unrelated, since
        // any guess would put the user on a photo they did not ask for.
        let Some(position) = visible.iter().position(|&i| i == current) else {
            return;
        };

        let len = visible.len() as isize;
        let next = ((position as isize + delta).rem_euclid(len)) as usize;
        self.open_modal(visible[next], ctx);
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

        // The modal pages through the grid as filtered, so its "3 / 40" counts
        // the visible photos and its position is a place in that list. A photo
        // that has left the view closes the modal rather than reporting a
        // position in a list it is no longer part of.
        let visible = self.visible_indices();
        let Some(position) = visible.iter().position(|&i| i == modal_index) else {
            self.modal_preview = None;
            return;
        };
        let item_count = visible.len();

        let is_loading = self.modal_preview.as_ref().is_some_and(|m| m.is_loading);
        let high_res_tex = self
            .modal_preview
            .as_ref()
            .and_then(|m| m.high_res_texture.clone());

        let mut close_modal = false;
        let mut prev_requested = false;
        let mut next_requested = false;
        let mut single_train_requested = false;

        // Split out so the closure below can borrow the item and the profiles
        // separately: `render_modal_controls` needs both.
        let filename = self.items[modal_index]
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let profiles = &self.profiles;
        let item = &mut self.items[modal_index];

        let modal_size = layout::inspection_modal_size(screen_rect.size());

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
                let actions = Self::render_modal_controls(profiles, ui, item, position, item_count);
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
