//! Окно-инспектор для выбранной audio-entity.
//!
//! Вынесено отдельно от `editor::ui`, потому что `ui.rs` уже
//! разросся. Вызывается из `editor_ui::draw`, если в выделении
//! есть entity с компонентом `AudioSource`.
//!
//! Изменения сразу применяются к `World`, плюс preview через
//! `EditorAction::PreviewSound` и «выбрать звук из файла» через
//! `EditorAction::LoadSound`.

use crate::ecs::{Entity, World};
use crate::editor::ui::AudioSnapshot;
use crate::editor::{EditorAction, EditorState};
use crate::game::audio::{AudioBus, AudioSource};

pub fn draw_window(
    ctx: &egui::Context,
    editor: &mut EditorState,
    world: &mut World,
    snapshot: &AudioSnapshot,
    action: &mut Option<EditorAction>,
) {
    let Some(entity) = snapshot.selected_source_entity else { return; };
    if !world.has::<AudioSource>(entity) { return; }

    let title = format!("🔊 AudioSource · #{}", entity);
    let mut open = true;
    egui::Window::new(title)
        .open(&mut open)
        .default_pos(egui::pos2(320.0, 120.0))
        .default_width(360.0)
        .resizable(true)
        .show(ctx, |ui| {
            let Some(src) = world.get_mut::<AudioSource>(entity) else {
                ui.label(egui::RichText::new("component removed").weak().italics());
                return;
            };

            // --- Sound ---
            ui.horizontal(|ui| {
                ui.label("Sound:");
                if snapshot.sound_names.is_empty() {
                    ui.label(
                        egui::RichText::new("(no sounds loaded — Assets → Audio)")
                            .weak()
                            .italics(),
                    );
                } else {
                    let current = src.sound.clone();
                    egui::ComboBox::from_id_salt("inspector_audio_sound")
                        .selected_text(&current)
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for name in &snapshot.sound_names {
                                if ui
                                    .selectable_label(current == *name, name)
                                    .clicked()
                                {
                                    src.sound = name.clone();
                                }
                            }
                        });
                    if ui
                        .small_button("▶")
                        .on_hover_text("Preview (non-spatial, SFX bus)")
                        .clicked()
                    {
                        *action = Some(EditorAction::PreviewSound(src.sound.clone()));
                    }
                    if ui
                        .small_button("📂")
                        .on_hover_text("Load sound from file…")
                        .clicked()
                    {
                        *action = Some(EditorAction::LoadSound);
                    }
                }
            });

            // --- Bus ---
            ui.horizontal(|ui| {
                ui.label("Bus:");
                let current = src.bus;
                egui::ComboBox::from_id_salt("inspector_audio_bus")
                    .selected_text(current.name())
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for bus in AudioBus::ALL {
                            if ui
                                .selectable_label(current == bus, bus.name())
                                .clicked()
                            {
                                src.bus = bus;
                            }
                        }
                    });
            });

            // --- Playback ---
            ui.horizontal(|ui| {
                ui.checkbox(&mut src.playing, "playing");
                ui.checkbox(&mut src.looping, "looping");
            });

            // --- Volume / Pitch ---
            ui.add(
                egui::Slider::new(&mut src.volume, 0.0..=2.0)
                    .text("volume")
                    .fixed_decimals(2),
            );
            ui.add(
                egui::Slider::new(&mut src.pitch, 0.25..=3.0)
                    .text("pitch")
                    .logarithmic(true)
                    .fixed_decimals(2),
            );

            // --- Spatial ---
            ui.separator();
            ui.label(egui::RichText::new("Spatial").strong());
            let spatial = src.max_distance > src.min_distance;
            ui.label(
                egui::RichText::new(if spatial {
                    "3D — attenuates by distance to listener"
                } else {
                    "2D — non-spatial (full volume everywhere)"
                })
                .small()
                .weak()
                .italics(),
            );
            ui.add(
                egui::Slider::new(&mut src.min_distance, 0.0..=30.0)
                    .text("min distance")
                    .fixed_decimals(2),
            );
            ui.add(
                egui::Slider::new(&mut src.max_distance, 0.0..=100.0)
                    .text("max distance")
                    .fixed_decimals(2),
            );
            if src.max_distance < src.min_distance {
                src.max_distance = src.min_distance;
            }
            ui.horizontal(|ui| {
                if ui
                    .small_button("Make non-spatial")
                    .on_hover_text("Sets max_distance = 0 (plays at full volume everywhere)")
                    .clicked()
                {
                    src.max_distance = 0.0;
                }
                if ui
                    .small_button("Default (1..20m)")
                    .clicked()
                {
                    src.min_distance = 1.0;
                    src.max_distance = 20.0;
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Remove AudioSource").clicked() {
                    world.remove::<AudioSource>(entity);
                }
                if ui.button("Focus (F)").clicked() {
                    editor.select_single(entity);
                    *action = Some(EditorAction::FocusSelected);
                }
            });
        });

    let _ = open;
}