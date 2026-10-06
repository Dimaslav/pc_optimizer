use glam::{Mat4, Vec3, Vec4};

const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Orbit,
    FirstPerson,
    Fly,
}

pub struct Camera3D {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
    pub mode: CameraMode,
    pub first_person_pos: Vec3,
}

impl Camera3D {
    pub fn new(aspect: f32) -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 8.0,
            yaw: 0.7,
            pitch: 0.5,
            fov_y: 60.0_f32.to_radians(),
            aspect,
            near: 0.1,
            far: 1000.0,
            mode: CameraMode::Orbit,
            first_person_pos: Vec3::new(0.0, 1.7, -8.0),
        }
    }

    pub fn set_viewport(&mut self, width: u32, height: u32) {
        if height > 0 {
            self.aspect = width as f32 / height as f32;
        }
    }

    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-cp * cy, -sp, -cp * sy)
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize_or_zero()
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize_or_zero()
    }

    pub fn position(&self) -> Vec3 {
        match self.mode {
            CameraMode::Orbit => {
                let (sy, cy) = self.yaw.sin_cos();
                let (sp, cp) = self.pitch.sin_cos();
                self.target
                    + Vec3::new(
                        self.distance * cp * cy,
                        self.distance * sp,
                        self.distance * cp * sy,
                    )
            }
            CameraMode::FirstPerson | CameraMode::Fly => self.first_person_pos,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        let pos = self.position();
        let look = match self.mode {
            CameraMode::Orbit => self.target,
            CameraMode::FirstPerson | CameraMode::Fly => pos + self.forward(),
        };
        Mat4::look_at_rh(pos, look, Vec3::Y)
    }

    pub fn proj_matrix(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far)
    }

    pub fn view_projection(&self) -> Mat4 {
        self.proj_matrix() * self.view_matrix()
    }

    // ============================================================
    // Orbit
    // ============================================================

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        if self.mode != CameraMode::Orbit {
            return;
        }
        // ИСПРАВЛЕНО: было self.yaw -= dx. Все три режима теперь
        // используют одинаковый знак — мышь вправо → поворот вправо.
        self.yaw += dx;
        self.pitch = (self.pitch + dy).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn zoom(&mut self, delta: f32) {
        if self.mode != CameraMode::Orbit {
            return;
        }
        self.distance = (self.distance * (1.0 - delta)).clamp(0.5, 500.0);
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        if self.mode != CameraMode::Orbit {
            return;
        }
        let view = self.view_matrix();
        let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
        let up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);
        self.target += (right * -dx + up * dy) * self.distance * 0.002;
    }

    pub fn focus_on(&mut self, target: Vec3, radius: f32) {
        if self.mode != CameraMode::Orbit {
            return;
        }
        self.target = target;
        self.distance = (radius * 2.5).max(1.0);
    }

    // ============================================================
    // FirstPerson (Play)
    // ============================================================

    pub fn enter_fps(&mut self, eye: Vec3) {
        if self.mode == CameraMode::FirstPerson {
            return;
        }
        self.mode = CameraMode::FirstPerson;
        self.first_person_pos = eye;
    }

    pub fn exit_fps(&mut self) {
        if self.mode != CameraMode::FirstPerson {
            return;
        }
        self.to_orbit();
    }

    pub fn is_first_person(&self) -> bool {
        self.mode == CameraMode::FirstPerson
    }

    pub fn fps_look(&mut self, dx: f32, dy: f32) {
        if self.mode != CameraMode::FirstPerson {
            return;
        }
        self.yaw += dx;
        self.pitch = (self.pitch + dy).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn fps_move(&mut self, delta: Vec3) {
        if self.mode != CameraMode::FirstPerson {
            return;
        }
        self.first_person_pos += delta;
    }

    // ============================================================
    // Fly (editor, UE5-style)
    // ============================================================

    pub fn enter_fly(&mut self, eye: Vec3) {
        if self.mode == CameraMode::Fly {
            return;
        }
        self.mode = CameraMode::Fly;
        self.first_person_pos = eye;
    }

    pub fn exit_fly(&mut self) {
        if self.mode != CameraMode::Fly {
            return;
        }
        self.to_orbit();
    }

    pub fn is_flying(&self) -> bool {
        self.mode == CameraMode::Fly
    }

    pub fn fly_look(&mut self, dx: f32, dy: f32) {
        if self.mode != CameraMode::Fly {
            return;
        }
        self.yaw += dx;
        self.pitch = (self.pitch + dy).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn fly_move(&mut self, delta: Vec3) {
        match self.mode {
            CameraMode::Fly | CameraMode::FirstPerson => {
                self.first_person_pos += delta;
            }
            CameraMode::Orbit => {
                self.target += delta;
            }
        }
    }

    fn to_orbit(&mut self) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let dir = Vec3::new(cp * cy, sp, cp * sy);
        self.target = self.first_person_pos - dir * self.distance;
        self.mode = CameraMode::Orbit;
    }

    pub fn frustum_planes(&self) -> [Vec4; 6] {
        let m = self.view_projection();
        [
            m.row(3) + m.row(0),
            m.row(3) - m.row(0),
            m.row(3) + m.row(1),
            m.row(3) - m.row(1),
            m.row(2),
            m.row(3) - m.row(2),
        ]
        .map(|p| {
            let n = Vec3::new(p.x, p.y, p.z).length();
            if n > 1e-8 { p / n } else { p }
        })
    }

    pub fn ray_from_screen(
        &self,
        screen_x: f32,
        screen_y: f32,
        width: f32,
        height: f32,
    ) -> (Vec3, Vec3) {
        let ndc_x = (screen_x / width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (screen_y / height) * 2.0;

        let inv_vp = self.view_projection().inverse();
        let near_h = inv_vp * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far_h = inv_vp * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);

        let near_w = near_h.truncate() / near_h.w;
        let far_w = far_h.truncate() / far_h.w;

        let origin = self.position();
        let dir = (far_w - near_w).normalize();
        (origin, dir)
    }

    pub fn project_to_screen(&self, p: Vec3, width: f32, height: f32) -> Option<(f32, f32)> {
        let clip = self.view_projection() * p.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let x = (ndc.x * 0.5 + 0.5) * width;
        let y = (1.0 - (ndc.y * 0.5 + 0.5)) * height;
        Some((x, y))
    }
}