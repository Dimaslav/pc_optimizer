//! Режимы отладочного отображения.
//!
//! Переключаются в `main.rs` клавишами F1–F6. Когда режим не `Final`,
//! bloom и ACES tonemap отключаются, и экран заполняется выбранной
//! текстурой напрямую.

#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum DebugView {
    /// Финальная картинка (bloom + ACES).
    Final = 0,
    /// SSAO после blur. Grayscale.
    Ssao = 1,
    /// G-buffer нормали (view-space). RGB remap [0,1].
    GbufferNormal = 2,
    /// G-buffer линейная глубина (нормализована по far). Grayscale.
    GbufferDepth = 3,
    /// HDR до bloom и tonemap. Сырой HDR клипается по [0,1] для отображения.
    HdrPreBloom = 4,
    /// Первый каскад CSM. Grayscale.
    CsmCascade0 = 5,
}

impl Default for DebugView {
    fn default() -> Self {
        Self::Final
    }
}

impl DebugView {
    /// Возвращает true, если нужен отдельный debug-pipeline.
    pub fn is_debug(self) -> bool {
        self != DebugView::Final
    }
}