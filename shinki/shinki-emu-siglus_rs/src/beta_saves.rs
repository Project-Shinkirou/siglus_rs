//! One-time import of saves written by ShinkiEMU beta.1/beta.2.
//!
//! Those betas redirected the whole game write tree (savedata, captures,
//! Twitter state) into `<app data>/siglus_rs/saves/<sha256(canonical game
//! path)>/siglus-<lang>-<identity>/`. The original engine and the patched CHS
//! build share `<game>/savedata/`, so every namespace of a game is merged back
//! into the game directory with its relative layout. Existing files always win;
//! an imported namespace is renamed so it is never imported again.
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

const IMPORTED_SUFFIX: &str = ".imported";

/// Import the default beta namespaces for `game`. A user-chosen beta
/// `save_root` is not recorded anywhere and has to be copied by hand.
pub(crate) fn import(game: &Path) -> io::Result<()> {
    let Some(app_data) = app_data() else {
        return Ok(());
    };
    let canonical = fs::canonicalize(game).map_err(|e| context(e, "canonicalize", game))?;
    let identity = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let root = app_data
        .join("siglus_rs")
        .join("saves")
        .join(format!("{identity:x}"));
    import_namespaces(&root, game)
}

/// The application-data base the betas used for their default save root.
fn app_data() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
    });
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        });
    #[cfg(not(any(windows, unix)))]
    let base: Option<PathBuf> = None;
    base
}

fn import_namespaces(root: &Path, game: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(context(e, "read", root)),
    };
    let mut namespaces = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| context(e, "read", root))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry
            .file_type()
            .map_err(|e| context(e, "inspect", &entry.path()))?
            .is_dir()
            || !name.starts_with("siglus-")
            || name.ends_with(IMPORTED_SUFFIX)
        {
            continue;
        }
        let path = entry.path();
        // On a conflict the namespace whose global data was written last wins.
        let written = fs::metadata(path.join("savedata").join("global.sav"))
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        namespaces.push((written, path));
    }
    namespaces.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, namespace) in namespaces {
        copy_tree(&namespace, game)?;
        let mut done = namespace.clone().into_os_string();
        done.push(IMPORTED_SUFFIX);
        fs::rename(&namespace, &done).map_err(|e| context(e, "mark imported", &namespace))?;
    }
    Ok(())
}

/// Copy regular files without replacing anything already in `to`.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to).map_err(|e| context(e, "create", to))?;
    for entry in fs::read_dir(from).map_err(|e| context(e, "read", from))? {
        let entry = entry.map_err(|e| context(e, "read", from))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| context(e, "inspect", &source))?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_file() {
            copy_file(&source, &target)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, target: &Path) -> io::Result<()> {
    let mut output = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(context(e, "create", target)),
    };
    let copied = fs::File::open(source)
        .map_err(|e| context(e, "open", source))
        .and_then(|mut input| {
            io::copy(&mut input, &mut output).map_err(|e| context(e, "write", target))
        })
        .and_then(|_| output.sync_all().map_err(|e| context(e, "write", target)));
    if copied.is_err() {
        drop(output);
        let _ = fs::remove_file(target);
    }
    copied.map(drop)
}

fn context(error: io::Error, action: &str, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("beta save import: {action} {}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("siglus-beta-import-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn beta_namespace_lands_in_the_original_game_layout_once() {
        let beta = Dir::new("layout-beta");
        let game = Dir::new("layout-game");
        let sav = [0x53, 0x41, 0x56, 0x00, 0xff, 0x10];
        beta.write("siglus-original-aa/savedata/0001.sav", &sav);
        beta.write("siglus-original-aa/savedata/global.sav", b"global");
        beta.write("siglus-original-aa/capture/shot.png", b"png");

        import_namespaces(&beta.0, &game.0).unwrap();

        assert_eq!(fs::read(game.0.join("savedata/0001.sav")).unwrap(), sav);
        assert_eq!(
            fs::read(game.0.join("savedata/global.sav")).unwrap(),
            b"global"
        );
        assert_eq!(fs::read(game.0.join("capture/shot.png")).unwrap(), b"png");
        assert!(!beta.0.join("siglus-original-aa").exists());
        assert!(
            beta.0
                .join("siglus-original-aa.imported/savedata/0001.sav")
                .is_file()
        );

        // A slot the player deletes after the import stays deleted.
        fs::remove_file(game.0.join("savedata/0001.sav")).unwrap();
        import_namespaces(&beta.0, &game.0).unwrap();
        assert!(!game.0.join("savedata/0001.sav").exists());
    }

    #[test]
    fn existing_game_saves_are_never_replaced() {
        let beta = Dir::new("noclobber-beta");
        let game = Dir::new("noclobber-game");
        game.write("savedata/global.sav", b"installed");
        beta.write("siglus-chs-bb/savedata/global.sav", b"beta");
        beta.write("siglus-chs-bb/savedata/0002.sav", b"slot");

        import_namespaces(&beta.0, &game.0).unwrap();

        assert_eq!(
            fs::read(game.0.join("savedata/global.sav")).unwrap(),
            b"installed"
        );
        assert_eq!(fs::read(game.0.join("savedata/0002.sav")).unwrap(), b"slot");
    }

    #[test]
    fn original_and_chs_namespaces_share_savedata_and_the_latest_global_wins() {
        let beta = Dir::new("shared-beta");
        let game = Dir::new("shared-game");
        beta.write("siglus-original-aa/savedata/global.sav", b"older");
        beta.write("siglus-original-aa/savedata/0001.sav", b"jp");
        beta.write("siglus-chs-bb/savedata/global.sav", b"newer");
        beta.write("siglus-chs-bb/savedata/0003.sav", b"cn");
        let older = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(beta.0.join("siglus-original-aa/savedata/global.sav"))
            .unwrap()
            .set_modified(older)
            .unwrap();

        import_namespaces(&beta.0, &game.0).unwrap();

        assert_eq!(
            fs::read(game.0.join("savedata/global.sav")).unwrap(),
            b"newer"
        );
        assert_eq!(fs::read(game.0.join("savedata/0001.sav")).unwrap(), b"jp");
        assert_eq!(fs::read(game.0.join("savedata/0003.sav")).unwrap(), b"cn");
    }
}
