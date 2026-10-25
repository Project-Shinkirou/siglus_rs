# Shinki modifications

## Sources

- Upstream `xmoezzz/siglus_rs` `5f358ef1360b0439e92aed2b749e8f25d8721954` (1.0.0).
- Stable release `1.0.0` still resolves to that upstream commit and is already an ancestor of `shinki`; no duplicate merge is needed. The adapter explicitly requires Rust 1.98 with edition 2024.
- Native baseline `13b55b47484faf8dfe03deb6ffd56ca0ec4ba3f2` originated on the `2018wzh/siglus_rs` fork: the native lifecycle and empty proc-stack fixes below, with the commercial hardware acceptance driver removed. The current fork is `Project-Shinkirou/siglus_rs`, branch `shinki`.
- Kira 0.9.6 stays the complete nested submodule `vendor/kira`: upstream `dfb08d08d065930e9b257203bf11c6d34742f542`, fix `a299b42690094a71885f6ed97dc504c4038da9a7` originally from `2018wzh/kira`. Its existing CPAL 0.15.3 WASAPI teardown pin `65ac8a6ac1a635cc26bfc5af331295b052044908` is unchanged.
- `shinki/shinki-emu-siglus_rs` depends on `shinki-emu-abi` (C ABI v3) and host-side `shinki-emu-build` from `Project-Shinkirou/ShinkiEMU@cb8d9dd22d7d0ab129085b324c94d6dc81fa19e3`. License texts and notices are in `shinki/THIRD_PARTY_NOTICES.md`.

## Native baseline fixes

- `platform_time` with the optional `pausable-clock` feature excludes suspended time using the real monotonic clock; replay-only virtual-clock is not enabled. `SiglusHost::set_suspended` pauses the original Kira mixer and clears input state.
- The Kira CPAL stream manager owns its JoinHandle, start-up returns device errors, and shutdown stops the device before joining. Suspension applies to the whole mixer; sound cursors, track fades and clocks do not catch up. The original device switch path is retained.
- Kira `DecoderOwner` and `StreamingSoundData::with_decoder_owner` keep the original decode, mixing and playback-handle drop semantics. Ordinary owner threads cancel and join every decoder, and one worker panic does not skip the other joins; realtime callbacks never join.
- `AudioHub` closes device/mixer before reclaiming decoders. The six MovieManager workers keep their decoders; receivers disconnect before join so bounded sends cannot block shutdown. Replaced voice decoders keep their owner until completion and are joined at close, never delivering stale results to a new voice.
- The WMV crate is version 0.2.1 with library name `siglus_wmv_decoder` (and its three CLI references) to avoid artifact name collisions with other cores.
- An exhausted native proc stack becomes idle instead of a permanent `paused` latch, so a `ReturnToMenu` queued by redraw still runs. Real suspension uses `set_suspended`.

## ShinkiEMU adapter

- `shinki/shinki-emu-siglus_rs` is a standalone Cargo workspace (own `Cargo.lock`) with package `shinki-emu-siglus_rs` and library `shinki_emu_siglus_rs` (`rlib` + `cdylib`). It holds the C ABI boundary, describe/probe, input mapping, native worker, typed platform bridge and immutable CPU leases. No adapter code remains in RoukakuEngine.
- Static registration uses `query`. The `dynamic` feature exports `shinki_emu_query` from `shinki_emu_siglus_rs.dll`, returning the same `CoreApi`, and enables `shinki-emu-abi/diagnostic-bridge` to install the shared log bridge; static builds reuse the host subscriber. `initialize` precedes describe/probe/open.
- Extension queries for `module_owner` and unknown extensions return `UNSUPPORTED` and clear the output pointer without touching global error or session state; later queries and successful calls do not inherit the rejection.
- The worker calls the original `SiglusHost::new_with_renderer_sync` / `step`, keeping the native proc stack, renderer, Kira, movies, input and savedata. The host passes no elapsed time and never advances the VM.
- Output keeps the original complete offscreen frame (sprite/wipe/shader/Emote/text/system UI) and its asynchronous single-staging readback. Three Arc CPU slots apply back-pressure; leases are not copied or extended to a fourth slot, and the five-second map deadline is not renewed by polling or resize.
- CPU leases are independent of session/device, stay valid after close and can be released on any thread. Their identity covers session/output/generation; successful native load/restart updates the context and cancels stale maps and platform requests.
- Raster and logical sizes use the selected Gameexe `SCREEN_SIZE`. Typed configuration provides `resource_language=original/chs` (default `original`) and `save_root` (default: the managed save root), rejecting unknown, duplicate and unsupported entries. Window size changes do not change the raster. CPU is the only output path; no text Hook is declared.
- The ABI 3 descriptor declares display name `siglus-rs`, engine `SiglusEngine` and `plugin_id` `siglus_rs`. It does not declare `CAP_RENDER_EXTENT` (`render` is zero) and open accepts only a 0×0 `render_extent`. Configuration uses Chinese labels, groups and descriptions: `resource_language` is “语言/资源语言” with choices “原版/简体中文”; `save_root` is “文件/存档目录” with `CONFIG_FLAG_DIRECTORY_PATH`, not relative to the game directory.
- One active session per process. Pause/focus/suspend are explicit events, cancel wakes platform waits independently, and open checks host cancellation. Close consumes the handle, stops production and reclaims the worker; handle queries never dereference a closed session. Native modal dialogs without a window return `UNSUPPORTED`.
- A game that ends itself (`GLOBAL.OWARI`, `SYSCOM.END_GAME` with or without its YES/NO warning, including the title or system menu exit button) runs the original EndGame proc: global data is written, `active_flag` clears, and `SiglusHost::step` returns `true` on that frame and every later one instead of resuming the suspended script. `SYSCOM.END_GAME` now ends the script proc at once, as `tnm_syscom_end_game()` does, so scene EOF cannot return to the menu first. The worker then publishes `SESSION_FINISHED` (flags 0), ignores later input and controls, and keeps the process alive; Manager close joins the worker as usual. A declined warning resumes the script.
- Input follows native desktop semantics: `NumpadEnter` and Meta use the original VM mapping; Insert, page keys, numpad and punctuation reach the real wait-key/direct-text path of `SiglusHost::key_event`. Committed text with an empty physical key calls the original `text_input`, keeping multi-character UTF-8 and the 32 KiB host limit. Unknown key names, buttons/events and stale frames are errors.
- Native errors use alternate Display to keep the `anyhow` cause chain. `module_owner` starts empty and native failures do not set `SESSION_CAN_RETRY`.

## Build

```
cargo build --manifest-path shinki/shinki-emu-siglus_rs/Cargo.toml --locked --release -p shinki-emu-siglus_rs --features dynamic
```

From the ShinkiEMU root, use `cargo xtask build --target x86_64-pc-windows-msvc --core siglus_rs --release`. The adapter's `build.rs` writes `OUT_DIR/core-build.json` with the target, profile, ABI and fork-relative license inputs, including the unchanged nested Kira licenses. License changes trigger regeneration; xtask reads the manifest and cdylib location from Cargo messages.

The MPEG/WMV decoders are statically linked; the DLL needs the Windows system libraries, a GPU driver and the VC runtime. Adapter tests cover key mapping, native unmapped-key dispatch, committed text limits, unknown-input rejection, stateless extension rejection, the native error cause chain and typed configuration. GPU-gated tests (`--include-ignored`) run generated public scenes: `crates/siglus_scene_vm/tests/host_exit.rs` drives `SiglusHost` through OWARI, END_GAME without warning, and an accepted or declined warning; the adapter's `session_tests.rs` opens a real ABI session, waits for `SESSION_FINISHED` after OWARI or a host-confirmed END_GAME warning, checks that the state stays finished, control is refused and global data was saved, and that close releases the core for a second session in the same process. Product acceptance is still manual in a Release host.

## Native resource language selection

The adapter's typed configuration exposes `resource_language = original | chs`, defaulting to `original`. Standalone hosts can select the same `ResourceLanguage` in `SiglusHostConfig`. The selection is immutable for a runtime and propagates to configuration tables, scene reloads/metadata, images, cursors, composed images, and movies.

`chs` requires both `Scene.chs` and `Gameexe.chs` in the active Select.ini append. It never substitutes either half from the original pair, another append, or Data/. Native readers decode the selected packages; invalid configuration and missing configured startup/menu scene references fail explicitly. CHS decoding uses explicit project keys without original-pair key recovery or input key persistence.

CHS image lookup prefers the matching G01 over G00 and decodes it with the existing complete G00 album reader, preserving frame numbering. Other image formats and original-mode extension order remain unchanged. Movie replacement uses `mov_chs` only for the exact filename/extension selected by the normal `mov` resolver in the same append. Unreplaced resources retain their original native paths.

Font discovery remains the existing `font`/`fonts` mechanism. No speculative font override or binary-offset patches are added. The Rewrite sample still requires an actual font with glyph coverage and established semantics for all five legacy machine-code patch sites; these native resource capabilities alone do not establish complete localization readiness. Anemoi separately lacks a confirmed translation source.

Public generated fixtures in `crates/siglus_scene_vm/tests/localization_resources.rs` cover paired native Scene/Gameexe parsing and strings, strict missing/incompatible pair errors, append boundaries, complete two-frame G01 decoding and rejection of an invalid later frame, and movie path precedence; the adapter tests cover typed ABI configuration. Tests do not execute a game VM.

## Managed save isolation

The ABI layout is unchanged. The adapter's typed schema adds `save_root`, a string with an empty default. An empty value selects platform application data (`LOCALAPPDATA`, macOS Application Support, or XDG data/HOME), namespaced by the canonical game path. Explicit roots must be absolute and outside the game tree and original save directories.

Each runtime then selects an immutable namespace containing the language and SHA-256 identity of its selected Scene/Gameexe pair. It never imports or migrates installation saves. Config/global/read/local saves, slot edits and copies, thumbnails, captures, backlog exports and Twitter state all use that location. Capture output paths cannot escape it. Text/capture readers reject traversal or links into the installation's original save directories, while normal game assets remain readable.

Managed sessions also use explicit read-only project key options, without original EXE-key or Emote-key persistence. A corrupt managed read-flag file is an initialization error instead of being replaced by default state. Save binary formats remain unchanged. Standalone original-mode hosts with no override retain their existing project-local save policy.

Public temporary-directory fixtures exercise native save round-trips, original/CHS/data identity separation, unchanged original saves, failed-read preservation, native PNG capture output, path rejection and FILE.LOAD_TXT save isolation. `sha2 = 0.10.9` is a native dependency recorded in both `Cargo.lock` files.
