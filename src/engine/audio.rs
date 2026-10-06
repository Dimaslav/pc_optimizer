//! Процедурный и файловый аудио-движок на базе `rodio` 0.21.
//!
//! # Возможности
//!
//! * Процедурные звуки (`shot`/`explosion`/`pickup`/`ding`) — синтез в
//!   памяти, ноль дисковых операций.
//! * Загрузка из файлов (Фаза 4.3): `.wav`, `.ogg`, `.flac`, `.mp3`.
//!   Файл декодируется один раз в `Vec<f32>` (моно) и кэшируется как
//!   `Arc` — `play` становится атомарной операцией.
//! * Spatial audio (Фаза 4.1): `AudioSource` компонент + distance
//!   attenuation + плавное сглаживание громкости.
//! * Шины (Фаза 4.2): Master / SFX / Music / Voice / UI. Громкость
//!   шины применяется поверх spatial.
//!
//! # Про формат сэмплов
//!
//! `rodio::Decoder` в 0.21 нормализует все форматы в `f32` (через
//! symphonia + dasp). Поэтому после `Decoder::new(...)` мы просто
//! собираем итератор в `Vec<f32>` — никакого `convert_samples` не
//! требуется. Проверено на wav / ogg / flac / mp3.
//!
//! # Про память
//!
//! Декодирование в память — стратегия для SFX (короткие, до 2-3 секунд).
//! Длинная музыка (5+ минут) съест десятки MB. Streaming — отдельная
//! итерация, требует собственного микшера (rodio не даёт seek-able
//! Source без full decode).
//!
//! # Про тесты
//!
//! `AudioSystem::new` открывает аудио-устройство — на CI его нет, и
//! `cargo test` упал бы. Поэтому юнит-тесты работают с чистой функцией
//! `effective_bus_volume`, а не с самим `AudioSystem`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use glam::Vec3;
use rodio::{OutputStreamBuilder, Sink, Source};

use crate::ecs::{Entity, World};
use crate::game::audio::{attenuation, source_position, AudioBus, AudioSource};

/// Кэш одного звука.
#[derive(Clone)]
struct SoundEntry {
    samples: Arc<Vec<f32>>,
    sample_rate: u32,
}

/// `rodio::Source` над `Arc<Vec<f32>>`.
struct SharedSamples {
    data: Arc<Vec<f32>>,
    pos: usize,
    sample_rate: u32,
}

impl Iterator for SharedSamples {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.data.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for SharedSamples {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Активный spatial-звук.
struct ActiveSound {
    sink: Sink,
    last_volume: f32,
    last_pitch: f32,
}

pub struct AudioSystem {
    _stream: rodio::OutputStream,

    cache: HashMap<String, SoundEntry>,

    /// Громкость шин. `AudioBus::Master` — глобальный множитель,
    /// применяется поверх шины звука (см. `effective_bus_volume`).
    bus_volumes: HashMap<AudioBus, f32>,

    active: HashMap<Entity, ActiveSound>,
}

/// Чистая формула эффективной громкости шины.
///
/// * Для `bus == Master` — только `volume(Master)` (иначе удваивали бы).
/// * Иначе — `volume(bus) * volume(Master)`.
pub fn effective_bus_volume(
    bus_volumes: &HashMap<AudioBus, f32>,
    bus: AudioBus,
) -> f32 {
    let v = bus_volumes.get(&bus).copied().unwrap_or(1.0);
    if bus == AudioBus::Master {
        v
    } else {
        v * bus_volumes.get(&AudioBus::Master).copied().unwrap_or(1.0)
    }
}

impl AudioSystem {
    pub fn new() -> Option<Self> {
        let stream = match OutputStreamBuilder::open_default_stream() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("audio: cannot open output stream: {}", e);
                return None;
            }
        };

        let sr = 44_100u32;
        let mut cache = HashMap::new();
        cache.insert("shot".into(), SoundEntry {
            samples: Arc::new(gen_shot(sr)),
            sample_rate: sr,
        });
        cache.insert("explosion".into(), SoundEntry {
            samples: Arc::new(gen_explosion(sr)),
            sample_rate: sr,
        });
        cache.insert("pickup".into(), SoundEntry {
            samples: Arc::new(gen_pickup(sr)),
            sample_rate: sr,
        });
        cache.insert("ding".into(), SoundEntry {
            samples: Arc::new(gen_ding(sr)),
            sample_rate: sr,
        });

        let bus_volumes = AudioBus::ALL
            .iter()
            .map(|b| (*b, b.default_volume()))
            .collect();

        log::info!(
            "audio: initialized @ {} Hz, {} procedural sounds, {} buses",
            sr,
            cache.len(),
            AudioBus::ALL.len(),
        );

        Some(Self {
            _stream: stream,
            cache,
            bus_volumes,
            active: HashMap::new(),
        })
    }

    // ============================================================
    // Регистрация звуков
    // ============================================================

    /// Зарегистрировать процедурный звук (name → samples).
    pub fn register(&mut self, name: impl Into<String>, samples: Vec<f32>) {
        let sr = 44_100u32;
        self.cache.insert(name.into(), SoundEntry {
            samples: Arc::new(samples),
            sample_rate: sr,
        });
    }

    /// Загрузка звука из файла (Фаза 4.3).
    ///
    /// Поддерживаются форматы, которые умеет `rodio::Decoder`:
    /// `.wav`, `.ogg`, `.flac`, `.mp3` (mp3 через symphonia).
    ///
    /// Файл декодируется целиком в память, стерео микшируется в моно.
    /// `rodio::Decoder` нормализует все сэмплы в `f32`, поэтому
    /// достаточно просто собрать итератор.
    pub fn load_sound_from_file(
        &mut self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<()> {
        let name: String = name.into();
        let path = path.as_ref();

        let file = std::fs::File::open(path)
            .with_context(|| format!("open audio {}", path.display()))?;
        let reader = std::io::BufReader::new(file);
        let decoder = rodio::Decoder::new(reader)
            .with_context(|| format!("decode audio {}", path.display()))?;

        let channels = decoder.channels() as usize;
        let sample_rate = decoder.sample_rate();

        // ИСПРАВЛЕНО: `Decoder::Item` уже `f32` — symphonia/dasp
        // нормализуют формат внутри rodio. Никакой конвертации не надо.
        let raw: Vec<f32> = decoder.collect();

        // Микшируем N каналов в моно.
        let samples = if channels > 1 {
            let frames = raw.len() / channels;
            let mut out = Vec::with_capacity(frames);
            let inv = 1.0 / channels as f32;
            for f in 0..frames {
                let mut sum = 0.0f32;
                for c in 0..channels {
                    sum += raw[f * channels + c];
                }
                out.push(sum * inv);
            }
            out
        } else {
            raw
        };

        if samples.is_empty() {
            bail!("decoded '{}' to empty sample buffer", path.display());
        }

        let dur_s = samples.len() as f32 / sample_rate.max(1) as f32;
        log::info!(
            "audio: loaded '{}' from {} ({:.2}s, {} Hz, {} channels → mono)",
            name,
            path.display(),
            dur_s,
            sample_rate,
            channels,
        );

        self.cache.insert(name, SoundEntry {
            samples: Arc::new(samples),
            sample_rate,
        });
        Ok(())
    }

    pub fn sound_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.cache.keys().map(|s| s.as_str()).collect();
        v.sort();
        v
    }

    pub fn has_sound(&self, name: &str) -> bool {
        self.cache.contains_key(name)
    }

    // ============================================================
    // Шины (Фаза 4.2)
    // ============================================================

    pub fn set_bus_volume(&mut self, bus: AudioBus, volume: f32) {
        let v = volume.clamp(0.0, 2.0);
        self.bus_volumes.insert(bus, v);
        log::debug!("audio: bus {} volume = {:.2}", bus.name(), v);
    }

    pub fn bus_volume(&self, bus: AudioBus) -> f32 {
        self.bus_volumes.get(&bus).copied().unwrap_or(1.0)
    }

    pub fn bus_volume_effective(&self, bus: AudioBus) -> f32 {
        effective_bus_volume(&self.bus_volumes, bus)
    }

    pub fn all_bus_volumes(&self) -> Vec<(AudioBus, f32)> {
        AudioBus::ALL.iter().map(|b| (*b, self.bus_volume(*b))).collect()
    }

    // ============================================================
    // Non-spatial воспроизведение
    // ============================================================

    pub fn play(&mut self, name: &str) {
        self.play_on_bus(name, AudioBus::Sfx);
    }

    pub fn play_on_bus(&mut self, name: &str, bus: AudioBus) {
        self.play_on_bus_with_volume(name, bus, 1.0);
    }

    pub fn play_on_bus_with_volume(&mut self, name: &str, bus: AudioBus, extra_volume: f32) {
        let Some(entry) = self.cache.get(name).cloned() else {
            log::debug!("audio: sound '{}' not found", name);
            return;
        };
        let volume = (extra_volume * self.bus_volume_effective(bus)).clamp(0.0, 4.0);
        if volume <= 1e-4 {
            return;
        }
        let source = SharedSamples {
            data: entry.samples,
            pos: 0,
            sample_rate: entry.sample_rate,
        };
        let sink = Sink::connect_new(self._stream.mixer());
        sink.set_volume(volume);
        sink.append(source);
        sink.detach();
    }

    // ============================================================
    // Spatial update
    // ============================================================

    pub fn update(&mut self, world: &mut World, listener_pos: Vec3, dt: f32) {
        // 1. Снимок компонентов.
        let sources: Vec<(Entity, AudioSource, Vec3)> = world
            .query::<AudioSource>()
            .map(|(e, src)| (e, src.clone(), source_position(world, e)))
            .collect();

        // 2. Cleanup.
        let alive: std::collections::HashSet<Entity> =
            sources.iter().map(|(e, _, _)| *e).collect();
        self.active.retain(|e, a| alive.contains(e) && !a.sink.empty());

        // 3. Обработка.
        for (e, src, pos) in sources {
            if !src.playing {
                if let Some(active) = self.active.remove(&e) {
                    active.sink.stop();
                }
                continue;
            }

            // 3a. Запуск нового звука.
            if !self.active.contains_key(&e) {
                let Some(entry) = self.cache.get(&src.sound).cloned() else {
                    log::debug!(
                        "audio: entity #{} wants '{}', not in cache",
                        e, src.sound
                    );
                    continue;
                };
                let source = SharedSamples {
                    data: entry.samples,
                    pos: 0,
                    sample_rate: entry.sample_rate,
                };
                let sink = Sink::connect_new(self._stream.mixer());
                let initial_vol = (src.volume * self.bus_volume_effective(src.bus))
                    .clamp(0.0, 4.0);
                sink.set_volume(initial_vol);
                sink.set_speed(src.pitch.max(0.01));

                if src.looping {
                    sink.append(source.repeat_infinite());
                } else {
                    sink.append(source);
                }

                self.active.insert(e, ActiveSound {
                    sink,
                    last_volume: initial_vol,
                    last_pitch: src.pitch,
                });
                continue;
            }

            // 3b. Обновление существующего.
            // ИСПРАВЛЕНО: bus_volume_effective требует &self, но active
            // держит &mut self. Вычисляем громкость шины заранее.
            let bus_vol = self.bus_volume_effective(src.bus);

            let Some(active) = self.active.get_mut(&e) else { continue };
            let dist = (pos - listener_pos).length();
            let atten = attenuation(dist, src.min_distance, src.max_distance);
            let target = (src.volume * atten * bus_vol).clamp(0.0, 4.0);

            let smoothing = (dt * 10.0).clamp(0.0, 1.0);
            let smoothed = active.last_volume + (target - active.last_volume) * smoothing;
            if (smoothed - active.last_volume).abs() > 1e-4 {
                active.sink.set_volume(smoothed);
                active.last_volume = smoothed;
            }

            if (src.pitch - active.last_pitch).abs() > 1e-4 {
                active.sink.set_speed(src.pitch.max(0.01));
                active.last_pitch = src.pitch;
            }
        }

        // 4. Обнуляем `playing` у завершившихся non-looping.
        let mut finished: Vec<Entity> = Vec::new();
        for (e, src) in world.query::<AudioSource>() {
            if !src.looping && src.playing && !self.active.contains_key(&e) {
                finished.push(e);
            }
        }
        for e in finished {
            if let Some(src) = world.get_mut::<AudioSource>(e) {
                src.playing = false;
            }
        }
    }

    pub fn stop_all(&mut self) {
        for (_, a) in self.active.drain() {
            a.sink.stop();
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

// ============================================================
// Процедурные генераторы
// ============================================================

fn gen_shot(sr: u32) -> Vec<f32> {
    let n = (sr as f32 * 0.08) as usize;
    let mut out = Vec::with_capacity(n);
    let mut seed: u32 = 0x1234_5678;
    for i in 0..n {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 8) & 0xFF_FFFF) as f32 / 16_777_215.0 * 2.0 - 1.0;
        let t = i as f32 / n as f32;
        let env = (1.0 - t).powi(3);
        out.push(noise * env * 0.55);
    }
    out
}

fn gen_explosion(sr: u32) -> Vec<f32> {
    let n = (sr as f32 * 0.6) as usize;
    let mut out = Vec::with_capacity(n);
    let mut seed: u32 = 0xCAFE_BABE;
    let mut lp = 0.0f32;
    for i in 0..n {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 8) & 0xFF_FFFF) as f32 / 16_777_215.0 * 2.0 - 1.0;
        lp = lp * 0.93 + noise * 0.07;
        let t = i as f32 / n as f32;
        let env = (1.0 - t).powi(2);
        out.push(lp * env * 1.8);
    }
    out
}

fn gen_pickup(sr: u32) -> Vec<f32> {
    let n = (sr as f32 * 0.25) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let freq = 600.0 + 900.0 * t;
        let phase = 2.0 * std::f32::consts::PI * freq * (i as f32 / sr as f32);
        let env = (1.0 - t).powi(2);
        out.push(phase.sin() * env * 0.45);
    }
    out
}

fn gen_ding(sr: u32) -> Vec<f32> {
    let n = (sr as f32 * 0.5) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let phase1 = 2.0 * std::f32::consts::PI * 880.0 * (i as f32 / sr as f32);
        let phase2 = 2.0 * std::f32::consts::PI * 1320.0 * (i as f32 / sr as f32);
        let env = (1.0 - t).powi(3);
        out.push((phase1.sin() * 0.4 + phase2.sin() * 0.2) * env);
    }
    out
}

// ============================================================
// Тесты
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_buses() -> HashMap<AudioBus, f32> {
        AudioBus::ALL
            .iter()
            .map(|b| (*b, b.default_volume()))
            .collect()
    }

    #[test]
    fn master_does_not_multiply_itself() {
        let mut buses = default_buses();
        buses.insert(AudioBus::Master, 0.5);
        assert!((effective_bus_volume(&buses, AudioBus::Master) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn bus_multiplied_by_master() {
        let mut buses = default_buses();
        buses.insert(AudioBus::Master, 0.5);
        buses.insert(AudioBus::Sfx, 1.0);
        assert!((effective_bus_volume(&buses, AudioBus::Sfx) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn zero_master_mutes_all() {
        let mut buses = default_buses();
        buses.insert(AudioBus::Master, 0.0);
        for b in AudioBus::ALL {
            assert!(
                effective_bus_volume(&buses, b) < 1e-6,
                "{:?} should be muted, got {}",
                b,
                effective_bus_volume(&buses, b),
            );
        }
    }

    #[test]
    fn missing_bus_falls_back_to_one() {
        let buses = HashMap::new();
        assert!((effective_bus_volume(&buses, AudioBus::Master) - 1.0).abs() < 1e-6);
        assert!((effective_bus_volume(&buses, AudioBus::Sfx) - 1.0).abs() < 1e-6);
    }
}