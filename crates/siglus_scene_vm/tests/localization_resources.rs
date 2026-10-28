//! Public generated data only. These tests parse resources without running a game VM.
use siglus_assets::scene_pck::ScenePck;
use siglus_scene_vm::{
    image_manager::ImageManager,
    resource::{self, ResourceLanguage},
    runtime::{constants::cd, tables::AssetTables, unknown::UnknownOpRecorder},
    scene_stream::SceneStream,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
#[path = "support/scene_fixture.rs"]
mod scene_fixture;
use scene_fixture::{gameexe, literals, put};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "siglus-public-localization-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn pair(&self) {
        self.write(
            "Scene.pck",
            &scene_pack(&["public original", "original choice"]),
        );
        self.write("Scene.chs", &scene_pack(&["公开译文", "公开选项"]));
        self.write("Gameexe.dat", &gameexe(640, "menu"));
        self.write("Gameexe.chs", &gameexe(1280, "menu"));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scene_chunk(strings: &[&str]) -> Vec<u8> {
    let mut code = vec![cd::NL];
    code.extend_from_slice(&42i32.to_le_bytes());
    code.push(cd::TEXT);
    code.extend_from_slice(&0i32.to_le_bytes());
    scene_fixture::scene_chunk(&code, strings)
}
fn scene_pack(strings: &[&str]) -> Vec<u8> {
    scene_fixture::scene_pack(&[
        ("start", scene_chunk(strings)),
        ("menu", scene_chunk(strings)),
    ])
}
fn g00_frames(colors: &[[u8; 4]], invalid_last_chip: bool) -> Vec<u8> {
    let mut decoded = vec![0; 4 + colors.len() * 8];
    put(&mut decoded, 0, colors.len());
    for (i, color) in colors.iter().enumerate() {
        let offset = decoded.len();
        let mut cut = vec![0; 116 + 92];
        cut[2..4].copy_from_slice(&1u16.to_le_bytes());
        for word in [3, 4, 7, 8] {
            put(&mut cut, word, 1);
        }
        let width: u16 = if invalid_last_chip && i + 1 == colors.len() {
            2
        } else {
            1
        };
        cut[116 + 6..116 + 8].copy_from_slice(&width.to_le_bytes());
        cut[116 + 8..116 + 10].copy_from_slice(&1u16.to_le_bytes());
        cut.extend_from_slice(color); // BGRA
        put(&mut decoded, 1 + i * 2, offset);
        put(&mut decoded, 2 + i * 2, cut.len());
        decoded.extend_from_slice(&cut);
    }
    let mut out = vec![2, 1, 0, 1, 0];
    out.extend_from_slice(&(colors.len() as u32).to_le_bytes());
    out.resize(9 + colors.len() * 24, 0);
    out.extend_from_slice(&literals(&decoded));
    out
}

#[test]
fn selected_pairs_use_native_gameexe_and_every_scene_string_without_vm_execution() {
    let f = Fixture::new();
    f.pair();
    let before: Vec<_> = ["Scene.pck", "Scene.chs", "Gameexe.dat", "Gameexe.chs"]
        .iter()
        .map(|p| fs::read(f.0.join(p)).unwrap())
        .collect();
    for (language, expected_width, expected) in [
        (
            ResourceLanguage::Original,
            640,
            ["public original", "original choice"],
        ),
        (ResourceLanguage::Chs, 1280, ["公开译文", "公开选项"]),
    ] {
        let decoded = resource::load_selected_gameexe(&f.0, language).unwrap();
        assert!(decoded.report.unwrap().used_lzss);
        let config = decoded.config;
        assert_eq!(
            config
                .get_entry("SCREEN_SIZE")
                .unwrap()
                .item_unquoted(0)
                .unwrap(),
            expected_width.to_string()
        );
        let options = language.scene_decode_options(&f.0).unwrap();
        let pack =
            ScenePck::load_lazy(&language.initial_scene_path(&f.0).unwrap(), &options).unwrap();
        resource::validate_selected_pair(language, &config, &pack).unwrap();
        for scene in 0..2 {
            let (owner, range) = pack.scn_data_shared(scene).unwrap();
            let mut stream =
                SceneStream::new_shared_range_with_string_codec(owner, range, pack.string_codec)
                    .unwrap();
            assert_eq!(stream.pop_u8().unwrap(), cd::NL);
            assert_eq!(stream.pop_i32().unwrap(), 42);
            assert_eq!(stream.pop_u8().unwrap(), cd::TEXT);
            assert_eq!(stream.pop_i32().unwrap(), 0);
            assert_eq!(stream.get_string(0).unwrap(), expected[0]);
            assert_eq!(stream.get_string(1).unwrap(), expected[1]);
            assert!(stream.get_string(2).is_err());
        }
        let mut notes = UnknownOpRecorder::default();
        let tables = AssetTables::load_with_language(&f.0, language, &mut notes).unwrap();
        assert_eq!(
            tables
                .gameexe
                .unwrap()
                .get_entry("SCREEN_SIZE")
                .unwrap()
                .item_unquoted(0)
                .unwrap(),
            expected_width.to_string()
        );
    }
    for (i, p) in ["Scene.pck", "Scene.chs", "Gameexe.dat", "Gameexe.chs"]
        .iter()
        .enumerate()
    {
        assert_eq!(fs::read(f.0.join(p)).unwrap(), before[i]);
    }
    assert!(!f.0.join("key.toml").exists());
}

#[test]
fn missing_chs_half_never_mixes_with_the_valid_original_pair() {
    for missing in ["Scene.chs", "Gameexe.chs"] {
        let f = Fixture::new();
        f.pair();
        fs::remove_file(f.0.join(missing)).unwrap();
        assert!(ResourceLanguage::Chs.initial_scene_path(&f.0).is_err());
        assert!(ResourceLanguage::Chs.initial_gameexe_path(&f.0).is_err());
        assert!(resource::load_selected_gameexe(&f.0, ResourceLanguage::Chs).is_err());
        assert!(ResourceLanguage::Original.initial_scene_path(&f.0).is_ok());
        assert!(resource::load_selected_gameexe(&f.0, ResourceLanguage::Original).is_ok());
    }
}

#[test]
fn incompatible_or_corrupt_chs_config_is_an_error_not_an_original_fallback() {
    let f = Fixture::new();
    f.pair();
    f.write("Gameexe.chs", &gameexe(1280, "absent_menu"));
    let decoded = resource::load_selected_gameexe(&f.0, ResourceLanguage::Chs).unwrap();
    let options = ResourceLanguage::Chs.scene_decode_options(&f.0).unwrap();
    let pack = ScenePck::load_lazy(
        &ResourceLanguage::Chs.initial_scene_path(&f.0).unwrap(),
        &options,
    )
    .unwrap();
    assert!(
        resource::validate_selected_pair(ResourceLanguage::Chs, &decoded.config, &pack).is_err()
    );
    f.write("Gameexe.chs", b"not a configuration");
    assert!(resource::load_selected_gameexe(&f.0, ResourceLanguage::Chs).is_err());
    let mut notes = UnknownOpRecorder::default();
    assert!(AssetTables::load_with_language(&f.0, ResourceLanguage::Chs, &mut notes).is_err());
    assert!(resource::load_selected_gameexe(&f.0, ResourceLanguage::Original).is_ok());
}

#[test]
fn chs_pair_is_exact_to_the_active_append_without_root_or_data_fallback() {
    let f = Fixture::new();
    f.pair();
    f.write("Select.ini", b"DLC\tPublic append\r\n");
    f.write("DLC/Scene.chs", &scene_pack(&["公开译文", "公开选项"]));
    assert!(ResourceLanguage::Chs.initial_scene_path(&f.0).is_err());
    f.write("DLC/Gameexe.chs", &gameexe(1280, "menu"));
    assert_eq!(
        ResourceLanguage::Chs.initial_scene_path(&f.0).unwrap(),
        f.0.join("DLC/Scene.chs")
    );
    assert_eq!(
        ResourceLanguage::Chs.initial_gameexe_path(&f.0).unwrap(),
        f.0.join("DLC/Gameexe.chs")
    );
}

#[test]
fn g01_selection_decodes_all_native_frames_and_keeps_original_defaults() {
    let f = Fixture::new();
    let original = g00_frames(&[[0, 255, 0, 255]], false);
    let localized = g00_frames(&[[0, 0, 255, 255], [255, 0, 0, 255]], false);
    f.write("g00/title.g00", &original);
    f.write("g00/title.g01", &localized);
    let parsed = siglus_assets::g00::G00::parse(&localized).unwrap();
    assert_eq!(parsed.cuts.len(), 2);
    assert_eq!(
        parsed.cuts[1].chips[0].decode_rgba().unwrap(),
        [0, 0, 255, 255]
    );
    let mut jp = ImageManager::new(f.0.clone());
    let jp_id = jp.load_g00("title", 0).unwrap();
    assert_eq!(jp.get(&jp_id).unwrap().rgba, [0, 255, 0, 255]);
    let mut cn = ImageManager::new_with_resource_language(f.0.clone(), ResourceLanguage::Chs);
    for (frame, expected) in [(0, [255, 0, 0, 255]), (1, [0, 0, 255, 255])] {
        let id = cn.load_g00("title.G00", frame).unwrap();
        assert_eq!(cn.get(&id).unwrap().rgba, expected);
    }
    assert!(cn.load_g00("title", 2).is_err());
    let (path, _) =
        resource::find_bg_image_with_append_dir(&f.0, "", "g00/title.g00", ResourceLanguage::Chs)
            .unwrap();
    assert_eq!(path, f.0.join("g00/title.g01"));
    assert_eq!(fs::read(f.0.join("g00/title.g00")).unwrap(), original);
}

#[test]
fn invalid_later_g01_frame_fails_even_when_only_frame_zero_is_requested() {
    let f = Fixture::new();
    f.write("g00/title.g00", &g00_frames(&[[0, 255, 0, 255]], false));
    f.write(
        "g00/title.g01",
        &g00_frames(&[[0, 0, 255, 255], [255, 0, 0, 255]], true),
    );
    let mut cn = ImageManager::new_with_resource_language(f.0.clone(), ResourceLanguage::Chs);
    assert!(cn.load_g00("title", 0).is_err());
    let mut jp = ImageManager::new(f.0.clone());
    assert!(jp.load_g00("title", 0).is_ok());
}

#[test]
fn localized_movie_resolution_replaces_only_the_exact_native_logical_movie() {
    let f = Fixture::new();
    f.write("mov/ED00.wmv", b"public original movie path fixture");
    f.write("mov_chs/ED00.wmv", b"public translated movie path fixture");
    f.write("mov/other.wmv", b"public unchanged movie path fixture");
    for name in ["ED00", "ed00.WMV", "mov/ED00.wmv"] {
        let (path, ty) =
            resource::find_mov_path_with_append_dir(&f.0, "", name, ResourceLanguage::Chs).unwrap();
        assert_eq!(
            fs::read(path).unwrap(),
            b"public translated movie path fixture"
        );
        assert_eq!(ty, resource::MovieType::Wmv);
    }
    let original = resource::find_mov_path(&f.0, "ED00").unwrap().0;
    assert_eq!(
        fs::read(original).unwrap(),
        b"public original movie path fixture"
    );
    let unchanged =
        resource::find_mov_path_with_append_dir(&f.0, "", "other", ResourceLanguage::Chs)
            .unwrap()
            .0;
    assert_eq!(
        fs::read(unchanged).unwrap(),
        b"public unchanged movie path fixture"
    );
    let append = Fixture::new();
    append.write("Select.ini", b"DLC\tPublic append\r\n");
    append.write(
        "mov_chs/ED00.wmv",
        b"public unrelated root translation fixture",
    );
    append.write(
        "DLC/mov/ED00.wmv",
        b"public append original movie path fixture",
    );
    let movie =
        resource::find_mov_path_with_append_dir(&append.0, "DLC", "ED00", ResourceLanguage::Chs)
            .unwrap()
            .0;
    assert_eq!(
        fs::read(movie).unwrap(),
        b"public append original movie path fixture"
    );
}
