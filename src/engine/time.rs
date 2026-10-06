use std::time::Instant;

pub struct Time {
    last: Instant,
    /// Сырой delta последнего кадра — для физики, анимаций, геймплея.
    ///
    /// ИЗМЕНЕНО (#11): именно это значение передаётся в
    /// `physics.step`, `game.update`, `update_player`,
    /// `update_particles`, `update_projectiles`.
    ///
    /// ИЗМЕНЕНО (Фаза GameState): при `paused == true` сюда пишется 0.0,
    /// но `delta_smooth` продолжает считаться от реального времени.
    /// Это даёт «заморозку» игровой логики при сохранении FPS-метрик.
    pub delta: f32,
    /// Сглаженный delta (скользящее среднее по 8 кадрам).
    pub delta_smooth: f32,
    pub elapsed: f32,
    pub frame_count: u64,
    fps_avg: f32,
    frame_time_max: f32,
    pub hitches: u64,

    /// ИЗМЕНЕНО (GameState): пауза. При `true` `tick()` возвращает
    /// `delta = 0.0`, но статистика FPS/сглаживание продолжают
    /// работать — чтобы UI паузы показывал актуальные показатели.
    ///
    /// Не путать с «остановкой времени»: `elapsed` не растёт, поэтому
    /// шейдеры, использующие `time` (skybox_time), «замерзают».
    /// Это правильное поведение для паузы.
    pub paused: bool,

    samples: [f32; SMOOTH_N],
    sample_idx: usize,
    sample_count: usize,

    warmup_frames: u32,
}

const SMOOTH_N: usize = 8;
const WARMUP_FRAMES: u32 = 3;

const MAX_DELTA: f32 = crate::physics::world::MAX_DT;
const DEFAULT_DT: f32 = 1.0 / 60.0;

impl Time {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            delta: DEFAULT_DT,
            delta_smooth: DEFAULT_DT,
            elapsed: 0.0,
            frame_count: 0,
            fps_avg: 60.0,
            frame_time_max: 0.0,
            hitches: 0,
            paused: false,
            samples: [DEFAULT_DT; SMOOTH_N],
            sample_idx: 0,
            sample_count: 0,
            warmup_frames: WARMUP_FRAMES,
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let raw_delta = (now - self.last).as_secs_f32();
        self.last = now;

        if self.warmup_frames > 0 {
            self.warmup_frames -= 1;
            self.delta = if self.paused { 0.0 } else { DEFAULT_DT };
            if !self.paused { self.elapsed += self.delta; }
            self.frame_count += 1;
            return;
        }

        // Реальный dt используется для сглаживания FPS даже при паузе —
        // иначе Stats показывает замороженное число, и непонятно,
        // действительно ли игра «жива».
        let real_delta = raw_delta.min(MAX_DELTA);

        // Игровой dt обнуляется при паузе.
        self.delta = if self.paused { 0.0 } else { real_delta };
        if !self.paused {
            self.elapsed += self.delta;
        }
        self.frame_count += 1;

        // Сглаживание работает по real_delta, чтобы Stats оставался
        // информативным.
        self.samples[self.sample_idx] = real_delta;
        self.sample_idx = (self.sample_idx + 1) % SMOOTH_N;
        if self.sample_count < SMOOTH_N {
            self.sample_count += 1;
        }

        let sum: f32 = self.samples[..self.sample_count].iter().sum();
        self.delta_smooth = sum / self.sample_count as f32;

        self.frame_time_max = self.samples[..self.sample_count]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);

        // Hitches считаем только когда не пауза — при паузе
        // «пропущенные кадры» не имеют смысла.
        if !self.paused
            && self.sample_count >= SMOOTH_N
            && real_delta > self.delta_smooth * 2.0
            && real_delta > 0.003
        {
            self.hitches += 1;
        }

        let inst_fps = if self.delta_smooth > 0.0001 {
            1.0 / self.delta_smooth
        } else {
            0.0
        };
        const ALPHA: f32 = 0.05;
        self.fps_avg = self.fps_avg * (1.0 - ALPHA) + inst_fps * ALPHA;
    }

    pub fn fps(&self) -> f32 { self.fps_avg }
    pub fn frame_time_max_ms(&self) -> f32 { self.frame_time_max * 1000.0 }
    pub fn frame_time_avg_ms(&self) -> f32 { self.delta_smooth * 1000.0 }
}

impl Default for Time {
    fn default() -> Self { Self::new() }
}