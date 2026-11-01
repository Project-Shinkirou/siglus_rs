//! Generated encrypted resources exercise startup without shipping game data.
use super::*;
use std::fs;

use crate::scene_fixture as fixture;

struct Project(PathBuf);
impl Project {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "siglus-encrypted-startup-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn encrypted_pair(key: &[u8; 16], width: usize) -> (Vec<u8>, Vec<u8>) {
    use siglus_scene_vm::formats::keys::SCENE_KEY;
    let mut game = fixture::gameexe(width, "menu");
    fixture::put(&mut game, 1, 1);
    for (i, byte) in game[8..].iter_mut().enumerate() {
        *byte ^= key[i % 16];
    }
    let scenes: Vec<_> = (0..8)
        .map(|i| {
            let mut code = fixture::Code::default();
            for line in 0..=i {
                code.line(line);
            }
            // The resource cracker accepts the original compiler order:
            // header, string index/data, bytecode, then trailing tables.
            // The lifecycle fixture intentionally uses a different easy-link
            // layout, so it cannot serve as a compressed recovery seed.
            let code = code.eof();
            let mut raw = vec![0; 132];
            raw.extend_from_slice(&code);
            fixture::put(&mut raw, 0, 132);
            fixture::put(&mut raw, 1, 132);
            fixture::put(&mut raw, 2, code.len());
            fixture::put(&mut raw, 3, 132);
            fixture::put(&mut raw, 5, 132);
            let end = raw.len();
            for word in [7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31] {
                fixture::put(&mut raw, word, end);
            }
            let mut packed = fixture::literals(&raw);
            for (j, byte) in packed.iter_mut().enumerate() {
                *byte ^= SCENE_KEY[j % 256] ^ key[j % 16];
            }
            (format!("scene{i}"), packed)
        })
        .collect();
    let refs: Vec<_> = scenes.iter().map(|(name, bytes)| (name.as_str(), bytes.clone())).collect();
    let mut scene = fixture::scene_pack(&refs);
    fixture::put(&mut scene, 21, 1);
    fixture::put(&mut scene, 22, 870);
    (game, scene)
}

#[test]
fn screen_size_recovers_missing_and_stale_keys_before_decode() {
    // Distinct generated keys and dimensions prevent a game-specific key or
    // fixed screen-size workaround from satisfying the regression.
    for (name, key, width, stale) in [
        ("missing", [0x36, 0x0f, 0xc9, 0x37, 0x2e, 0xba, 0x09, 0xdc,
                     0xe4, 0x0d, 0xf2, 0x00, 0x23, 0xa3, 0x6e, 0x94], 1600, false),
        ("stale", [0x19, 0x42, 0xa8, 0x17, 0x91, 0x34, 0x20, 0xed,
                   0x78, 0x99, 0xab, 0xce, 0x27, 0x01, 0x64, 0x53], 1920, true),
    ] {
        let project = Project::new(name);
        let (game, scene) = encrypted_pair(&key, width);
        fs::write(project.0.join("Gameexe.dat"), &game).unwrap();
        fs::write(project.0.join("Scene.pck"), &scene).unwrap();
        if stale {
            fs::write(project.0.join("key.toml"), "key = \"00000000000000000000000000000000\"\n").unwrap();
        }
        assert_eq!(screen_size(&project.0, resource::ResourceLanguage::Original).unwrap(), (width as u32, 720));
        let persisted = fs::read_to_string(project.0.join("key.toml")).unwrap();
        assert_eq!(siglus_scene_vm::formats::key_toml::parse_key16_toml(&persisted).unwrap(), Some(key));
        assert_eq!(fs::read(project.0.join("Gameexe.dat")).unwrap(), game);
        assert_eq!(fs::read(project.0.join("Scene.pck")).unwrap(), scene);
        assert_eq!(screen_size(&project.0, resource::ResourceLanguage::Original).unwrap(), (width as u32, 720));
    }
}

#[test]
fn chs_does_not_recover_or_persist_against_original_pair() {
    let project = Project::new("chs");
    let key = [0x5a; 16];
    let (game, scene) = encrypted_pair(&key, 1600);
    for name in ["Gameexe.dat", "Gameexe.chs"] {
        fs::write(project.0.join(name), &game).unwrap();
    }
    for name in ["Scene.pck", "Scene.chs"] {
        fs::write(project.0.join(name), &scene).unwrap();
    }
    assert!(screen_size(&project.0, resource::ResourceLanguage::Chs).is_err());
    assert!(!project.0.join("key.toml").exists());
    for name in ["Gameexe.dat", "Gameexe.chs"] {
        assert_eq!(fs::read(project.0.join(name)).unwrap(), game);
    }
}

#[test]
fn configured_key_and_unencrypted_resources_keep_native_dimensions() {
    let project = Project::new("configured");
    let key = [0x61; 16];
    let (game, scene) = encrypted_pair(&key, 2048);
    fs::write(project.0.join("Gameexe.dat"), game).unwrap();
    fs::write(project.0.join("Scene.pck"), scene).unwrap();
    let configured = "key = \"61616161616161616161616161616161\"\n";
    fs::write(project.0.join("key.toml"), configured).unwrap();
    assert_eq!(screen_size(&project.0, resource::ResourceLanguage::Original).unwrap(), (2048, 720));
    assert_eq!(fs::read_to_string(project.0.join("key.toml")).unwrap(), configured);

    let plain = Project::new("plain");
    fs::write(plain.0.join("Gameexe.dat"), fixture::gameexe(800, "menu")).unwrap();
    assert_eq!(screen_size(&plain.0, resource::ResourceLanguage::Original).unwrap(), (800, 720));
    assert!(!plain.0.join("key.toml").exists());
}
