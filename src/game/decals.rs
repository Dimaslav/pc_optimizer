//! Компонент Decal — «наклейка» на поверхность.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decal {
    /// Имя текстуры в Renderer::textures.
    /// Обычно RGBA, где alpha = форма наклейки.
    pub texture: String,
    /// RGB tint поверх текстуры. Alpha = общая прозрачность.
    pub tint: [f32; 4],
}

impl Default for Decal {
    fn default() -> Self {
        Self {
            texture: "white".into(),
            tint: [1.0, 1.0, 1.0, 1.0],
        }
    }
}