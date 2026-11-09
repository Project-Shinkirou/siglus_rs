use super::{Failure, Result, abi, boundary, initialized};
use siglus_scene_vm::resource;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

const fn text(s: &'static str) -> abi::Bytes {
    abi::Bytes {
        data: s.as_ptr(),
        len: s.len(),
    }
}
struct Formats([abi::Bytes; 1]);
// All spans point to immutable library-lifetime literals.
unsafe impl Sync for Formats {}
static FORMATS: Formats = Formats([text("siglus.scene_pck")]);
struct LanguageChoices([abi::Bytes; 2]);
// Immutable library-lifetime spans, as for FORMATS.
unsafe impl Sync for LanguageChoices {}
static LANGUAGE_CHOICES: LanguageChoices = LanguageChoices([text("original"), text("chs")]);
static LANGUAGE_LABELS: LanguageChoices = LanguageChoices([text("原版"), text("简体中文")]);
static AUDIO_CHOICES: LanguageChoices = LanguageChoices([text("native"), text("nullaudiodevice")]);
struct Fields([abi::ConfigField; 3]);
unsafe impl Sync for Fields {}
static FIELDS: Fields = Fields([
    abi::ConfigField {
        id: text("resource_language"),
        label: text("资源语言"),
        group: text("语言"),
        default_value: abi::ConfigValue {
            kind: abi::VALUE_ENUM,
            reserved: 0,
            integer: 0,
            number: 0.0,
            text: text("original"),
        },
        integer_min: 0,
        integer_max: 0,
        number_min: 0.0,
        number_max: 0.0,
        max_bytes: 0,
        flags: 0,
        choices: LANGUAGE_CHOICES.0.as_ptr(),
        choice_count: LANGUAGE_CHOICES.0.len(),
        description: text(
            "选择读取的剧本与图像资源；简体中文需要游戏目录内成对的 Scene.chs 与 Gameexe.chs。",
        ),
        choice_labels: LANGUAGE_LABELS.0.as_ptr(),
        step: 0.0,
        unit: text(""),
    },
    abi::ConfigField {
        id: text("save_root"),
        label: text("存档目录"),
        group: text("文件"),
        default_value: abi::ConfigValue {
            kind: abi::VALUE_STRING,
            reserved: 0,
            integer: 0,
            number: 0.0,
            text: text(""),
        },
        integer_min: 0,
        integer_max: 0,
        number_min: 0.0,
        number_max: 0.0,
        max_bytes: 4096,
        flags: abi::CONFIG_FLAG_DIRECTORY_PATH,
        choices: std::ptr::null(),
        choice_count: 0,
        description: text(
            "存放存档的目录；留空时使用 ShinkiEMU 为该作品单独管理的存档目录，路径不相对于游戏目录。",
        ),
        choice_labels: std::ptr::null(),
        step: 0.0,
        unit: text(""),
    },
    abi::ConfigField {
        id: text("headless_audio_device"),
        label: text("Headless audio device"),
        group: text("Headless"),
        default_value: abi::ConfigValue {
            kind: abi::VALUE_ENUM,
            reserved: 0,
            integer: 0,
            number: 0.0,
            text: text("native"),
        },
        choices: AUDIO_CHOICES.0.as_ptr(),
        choice_count: AUDIO_CHOICES.0.len(),
        integer_min: 0,
        integer_max: 0,
        number_min: 0.0,
        number_max: 0.0,
        max_bytes: 0,
        flags: 0,
        description: text("Explicit isolated-headless output; normal sessions use native audio."),
        choice_labels: std::ptr::null(),
        step: 0.0,
        unit: text(""),
    },
]);

#[derive(Debug)]
pub(super) struct EmbeddedConfig {
    pub resource_language: resource::ResourceLanguage,
    pub save_root: Option<PathBuf>,
}

pub(super) unsafe fn parse(entries: &[abi::ConfigEntry]) -> Result<EmbeddedConfig> {
    unsafe { abi::validate_config(&FIELDS.0, entries) }.map_err(|e| {
        Failure::new(
            abi::INVALID_ARGUMENT,
            "SHINKI_EMU_SIGLUS_CONFIG",
            format!("configuration {}: {}", e.index, e.reason),
        )
    })?;
    let mut config = EmbeddedConfig {
        resource_language: resource::ResourceLanguage::Original,
        save_root: None,
    };
    for entry in entries {
        match unsafe { entry.id.text(128)? } {
            "resource_language" => {
                config.resource_language = match unsafe { entry.value.text.text(128)? } {
                    "original" => resource::ResourceLanguage::Original,
                    "chs" => resource::ResourceLanguage::Chs,
                    _ => return Err(abi::INVALID_ARGUMENT.into()),
                };
            }
            "save_root" => {
                let value = unsafe { entry.value.text.text(4096)? };
                if !value.is_empty() {
                    config.save_root = Some(PathBuf::from(value));
                }
            }
            "headless_audio_device" => {
                if unsafe { entry.value.text.text(128)? } != "nullaudiodevice" || !headless_audio_requested() {
                    return Err(Failure::new(abi::INVALID_ARGUMENT, "SHINKI_EMU_HEADLESS_AUDIO_DEVICE",
                        "NullAudioDevice requires an explicit isolated headless selection"));
                }
            }
            _ => return Err(abi::INVALID_ARGUMENT.into()),
        }
    }
    Ok(config)
}

fn headless_audio_requested() -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    { siglus_scene_vm::audio::headless_backend::requested() }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    { false }
}

static PROBE_ID: Mutex<String> = Mutex::new(String::new());
fn descriptor() -> abi::Descriptor {
    abi::Descriptor {
        header: abi::Header::of::<abi::Descriptor>(),
        family_id: text("siglus"),
        plugin_id: text("siglus_rs"),
        version: text(env!("CARGO_PKG_VERSION")),
        capabilities: abi::CAP_CPU_SURFACE,
        formats: FORMATS.0.as_ptr(),
        format_count: 1,
        configuration: FIELDS.0.as_ptr(),
        configuration_count: if headless_audio_requested() { FIELDS.0.len() } else { FIELDS.0.len() - 1 },
        display_name: text("siglus-rs"),
        family_name: text("SiglusEngine"),
        render: abi::RenderExtentSupport::default(),
    }
}
pub(super) unsafe extern "C" fn describe(
    out: *mut abi::Descriptor,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        initialized()?;
        let out = unsafe { out.as_mut() }.ok_or(abi::INVALID_ARGUMENT)?;
        *out = descriptor();
        Ok(())
    })
}
pub(super) unsafe extern "C" fn probe(
    path: abi::Bytes,
    out: *mut abi::ProbeResult,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        initialized()?;
        let out = unsafe { out.as_mut() }.ok_or(abi::INVALID_ARGUMENT)?;
        let root = Path::new(unsafe { path.text(32768)? });
        if !root.is_dir()
            || resource::find_initial_gameexe_path(root).is_err()
            || resource::find_scene_pck_path(root).is_err()
        {
            return Err(abi::NO_MATCH.into());
        }
        let mut id = PROBE_ID.lock().map_err(|_| abi::PANICKED)?;
        *id = root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(abi::INVALID_ARGUMENT)?
            .to_owned();
        *out = abi::ProbeResult {
            game_id: abi::Bytes::new(id.as_bytes()),
            format: text("siglus.scene_pck"),
            confidence_permyriad: 10000,
            reserved: 0,
        };
        Ok(())
    })
}
pub(super) fn screen_size(root: &Path, language: resource::ResourceLanguage) -> Result<(u32, u32)> {
    let config = resource::load_selected_gameexe(root, language, false)
        .map_err(Failure::native)?
        .config;
    let Some(entry) = config.get_entry("SCREEN_SIZE") else {
        return Ok((1280, 720));
    };
    let dimension = |index| {
        entry
            .item_unquoted(index)
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|n| (1..=16384).contains(n))
            .ok_or_else(|| {
                Failure::new(
                    abi::INVALID_ARGUMENT,
                    "SHINKI_EMU_SIGLUS_SCREEN_SIZE",
                    "invalid native SCREEN_SIZE",
                )
            })
    };
    Ok((dimension(0)?, dimension(1)?))
}
pub(super) fn surface(width: u32, height: u32, logical: (u32, u32)) -> Result<abi::SurfaceInfo> {
    let info = abi::SurfaceInfo {
        kind: abi::SURFACE_CPU,
        pixel_format: abi::PIXEL_RGBA8,
        color: abi::COLOR_SRGB,
        alpha: abi::ALPHA_IGNORED,
        rows: abi::ROWS_TOP_DOWN,
        logical: abi::Extent {
            width: logical.0,
            height: logical.1,
        },
        raster: abi::Extent { width, height },
        stride: u64::from(width) * 4,
    };
    info.validate()?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(value: &'static str) -> abi::ConfigEntry {
        abi::ConfigEntry {
            id: text("resource_language"),
            value: abi::ConfigValue {
                kind: abi::VALUE_ENUM,
                reserved: 0,
                integer: 0,
                number: 0.0,
                text: text(value),
            },
        }
    }

    #[test]
    fn language_schema_is_typed_and_original_by_default() {
        unsafe { abi::validate_config(&FIELDS.0, &[]) }.unwrap();
        let default = unsafe { parse(&[]) }.unwrap();
        assert_eq!(
            default.resource_language,
            resource::ResourceLanguage::Original
        );
        assert!(default.save_root.is_none());
        assert_eq!(
            unsafe { parse(&[entry("original")]) }
                .unwrap()
                .resource_language,
            resource::ResourceLanguage::Original
        );
        assert_eq!(
            unsafe { parse(&[entry("chs")]) }.unwrap().resource_language,
            resource::ResourceLanguage::Chs
        );
    }

    #[test]
    fn invalid_language_type_choice_or_duplicate_is_rejected() {
        let mut wrong_type = entry("chs");
        wrong_type.value.kind = abi::VALUE_STRING;
        assert!(unsafe { parse(&[wrong_type]) }.is_err());
        assert!(unsafe { parse(&[entry("auto")]) }.is_err());
        assert!(unsafe { parse(&[entry("original"), entry("chs")]) }.is_err());
        let mut unknown = entry("chs");
        unknown.id = text("unknown_language");
        assert!(unsafe { parse(&[unknown]) }.is_err());
    }

    #[test]
    fn save_root_is_a_typed_string_and_does_not_hide_language_selection() {
        let mut root = entry("D:/public-save-root");
        root.id = text("save_root");
        root.value.kind = abi::VALUE_STRING;
        for entries in [[root, entry("chs")], [entry("chs"), root]] {
            let config = unsafe { parse(&entries) }.unwrap();
            assert_eq!(config.resource_language, resource::ResourceLanguage::Chs);
            assert_eq!(config.save_root, Some(PathBuf::from("D:/public-save-root")));
        }
        root.value.kind = abi::VALUE_ENUM;
        assert!(unsafe { parse(&[root]) }.is_err());
    }
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod encrypted_tests;
