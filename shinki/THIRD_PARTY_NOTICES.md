# Third-party notices

## siglus_rs

Unofficial Rust reimplementation of SiglusEngine from
[`xmoezzz/siglus_rs`](https://github.com/xmoezzz/siglus_rs), upstream revision
`5f358ef1360b0439e92aed2b749e8f25d8721954` (1.0.0), as adapted on the `shinki`
branch of [`Project-Shinkirou/siglus_rs`](https://github.com/Project-Shinkirou/siglus_rs).
The native baseline is `13b55b47484faf8dfe03deb6ffd56ca0ec4ba3f2`.
The covered source, including the
`shinki/shinki-emu-siglus_rs` adapter, is distributed under the MPL-2.0; the
complete license text is `LICENSE-MPL-2.0` at the repository root. The change
inventory is `shinki/MODIFICATIONS.md`. Both files and the covered source must
remain in the source archive or the valid source offer accompanying a binary
distribution of `shinki_emu_siglus_rs.dll`.

## Vendored components

- `vendor/eluna_rs`: crates.io `eluna_rs` 0.1.0, MPL-2.0, license text at
  `vendor/eluna_rs/LICENSE-MPL-2.0`.
- `vendor/kira`: Kira 0.9.6 as a complete submodule, upstream
  `dfb08d08d065930e9b257203bf11c6d34742f542` with native worker-ownership fix
  `a299b42690094a71885f6ed97dc504c4038da9a7` from `2018wzh/kira`, dual
  licensed MIT / Apache-2.0 (`vendor/kira/LICENSE-MIT`,
  `vendor/kira/LICENSE-APACHE`).
- CPAL 0.15.3 git dependency `2018wzh/cpal@65ac8a6ac1a635cc26bfc5af331295b052044908`
  (WASAPI teardown fix), Apache-2.0, license shipped in its source.

## ShinkiEMU

`shinki-emu-abi` and the host-side `shinki-emu-build` come from
[`Project-Shinkirou/ShinkiEMU@cb8d9dd22d7d0ab129085b324c94d6dc81fa19e3`](https://github.com/Project-Shinkirou/ShinkiEMU/commit/cb8d9dd22d7d0ab129085b324c94d6dc81fa19e3),
MIT OR Apache-2.0.

## RoukakuEngine

The explicit desktop-headless output reuses `roukaku-platform-common::NullAudioDevice`
and its bounded PCM lane from
[`Project-Shinkirou/RoukakuEngine@374a7a20780153766d9d0256e4cf4d7754fd02f9`](https://github.com/Project-Shinkirou/RoukakuEngine/commit/374a7a20780153766d9d0256e4cf4d7754fd02f9),
MIT OR Apache-2.0. Cargo resolves the original implementation, rather than a
copied device or a mock mixer. The corresponding-source and Rust-license
collection include these pinned packages and their license texts.

## Registry dependencies

Registry crates (wgpu 0.20, winit 0.31.0-beta.3, egui 0.28, ab_glyph,
encoding_rs, sha2 and their transitive closure) are resolved from crates.io and
carry their own licenses. `shinki/shinki-emu-siglus_rs/Cargo.lock` records the set
compiled into the adapter.

## Key material

Protected retail titles need the SiglusEngine key supplied by the user in the
game directory's `key.toml`. No key material or key-extraction tooling is
distributed.
