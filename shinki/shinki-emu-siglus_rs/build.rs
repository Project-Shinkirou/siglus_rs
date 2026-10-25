use shinki_emu_build::{BuildAsset, CoreBuildManifest, write_core_build_manifest};
use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing CARGO_MANIFEST_DIR")?)
        .join("../..")
        .canonicalize()?;
    let mut manifest = CoreBuildManifest::new("siglus_rs", "rust-cdylib")?;
    if manifest.target.ends_with("-apple-darwin") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libshinki_emu_siglus_rs.dylib");
    }
    for path in [
        "LICENSE-MPL-2.0",
        "shinki/THIRD_PARTY_NOTICES.md",
        "shinki/MODIFICATIONS.md",
        "vendor/kira/LICENSE-MIT",
        "vendor/kira/LICENSE-APACHE",
    ] {
        manifest.licenses.push(BuildAsset {
            source: root.join(path),
            dest: format!("licenses/siglus_rs/{path}"),
        });
    }
    write_core_build_manifest(&manifest)?;
    Ok(())
}
