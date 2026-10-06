//! Загрузка исходников шейдеров.
//!
//! В debug-сборке файлы читаются с диска — это основа hot-reload.
//! В release-сборке содержимое вшивается в бинарь через `include_str!`.
//!
//! Пути указываются **относительно корня проекта** (например,
//! `"src/render/shaders/gbuffer.wgsl"`), потому что оба режима
//! отталкиваются от `CARGO_MANIFEST_DIR`.

#[cfg(debug_assertions)]
pub fn read(rel: &str) -> Result<String, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read shader '{}': {}", path.display(), e))
}

/// Загружает шейдер.
///
/// debug → читает файл с диска (hot-reload работает).
/// release → вшивает содержимое через `include_str!`.
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! shader_source {
    ($rel:literal) => {
        $crate::render::shader_source::read($rel)
    };
}

#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! shader_source {
    ($rel:literal) => {
        ::std::result::Result::<::std::string::String, ::std::string::String>::Ok(
            ::std::string::String::from(
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $rel))
            )
        )
    };
}