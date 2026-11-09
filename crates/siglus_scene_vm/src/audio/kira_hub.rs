use std::fmt;
#[cfg(feature = "pausable-clock")]
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(any(target_os = "horizon", target_os = "vita"))]
use super::switch_backend::SwitchBackend as PlatformBackend;
use anyhow::{Context, Result, anyhow};
use kira::Volume;
#[cfg(all(feature = "native-audio", any(target_os = "windows", target_os = "macos", target_os = "linux")))]
use super::headless_backend::DesktopBackend as PlatformBackend;
#[cfg(all(not(any(target_os = "horizon", target_os = "vita")), not(all(feature = "native-audio", any(target_os = "windows", target_os = "macos", target_os = "linux")))))]
use kira::manager::backend::DefaultBackend as PlatformBackend;
use kira::manager::{AudioManager, AudioManagerSettings};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
#[cfg(not(target_arch = "wasm32"))]
use kira::sound::streaming::{DecoderOwner, StreamingSoundData, StreamingSoundHandle};
use kira::track::{TrackBuilder, TrackHandle};
use kira::tween::Tween;

impl AudioHub {
    fn default_volumes() -> Self {
        Self {
            manager: None,
            #[cfg(not(target_arch = "wasm32"))]
            decoders: DecoderOwner::new(),
            initialization_error: None,
            bgm: None,
            se: None,
            pcm: None,
            koe: None,
            mov: None,
            bgm_base: 255,
            se_base: 255,
            pcm_base: 255,
            koe_base: 255,
            mov_base: 255,
            bgm_master: 255,
            se_master: 255,
            pcm_master: 255,
            koe_master: 255,
            mov_master: 255,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Bgm,
    Se,
    Pcm,
    Koe,
    Mov,
}


/// Shared Kira backend.
///
/// Kira 0.9 uses `AudioManager::play` plus `StaticSoundData::output_destination(&track)`.
/// We keep one sub-track per Siglus category (BGM/SE/PCM).
pub struct AudioHub {
    manager: Option<AudioManager<PlatformBackend>>,
    // Field order is intentional: device/mixer stops before decoder joins.
    #[cfg(not(target_arch = "wasm32"))]
    decoders: DecoderOwner,
    initialization_error: Option<&'static str>,
    bgm: Option<TrackHandle>,
    se: Option<TrackHandle>,
    pcm: Option<TrackHandle>,
    koe: Option<TrackHandle>,
    mov: Option<TrackHandle>,
    bgm_base: u8,
    se_base: u8,
    pcm_base: u8,
    koe_base: u8,
    mov_base: u8,
    bgm_master: u8,
    se_master: u8,
    pcm_master: u8,
    koe_master: u8,
    mov_master: u8,
}

impl fmt::Debug for AudioHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioHub")
            .field("enabled", &self.manager.is_some())
            .finish()
    }
}

impl Default for AudioHub {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioHub {
    #[cfg(feature = "pausable-clock")]
    pub fn set_suspended(&mut self, suspended: bool) {
        if let Some(manager) = self.manager.as_mut() {
            manager.backend_mut().set_suspended(suspended);
        }
    }
    #[cfg(feature = "pausable-clock")]
    pub fn suspension_state(&mut self) -> Result<Arc<AtomicBool>> {
        let manager = self.manager.as_mut().ok_or_else(|| anyhow!("audio disabled"))?;
        Ok(manager.backend_mut().suspension_state())
    }

    pub fn new() -> Self {
        #[cfg(feature = "native-audio")]
        let manager = if cfg!(feature = "virtual-clock") {
            Err(anyhow!("virtual clock: audio off"))
        } else {
            AudioManager::<PlatformBackend>::new(AudioManagerSettings::default())
                .map_err(|error| anyhow!("{error}"))
        };
        #[cfg(feature = "native-audio")]
        match manager {
            Ok(mut manager) => {
                let bgm = manager.add_sub_track(TrackBuilder::default()).ok();
                let se = manager.add_sub_track(TrackBuilder::default()).ok();
                let pcm = manager.add_sub_track(TrackBuilder::default()).ok();
                let koe = manager.add_sub_track(TrackBuilder::default()).ok();
                let mov = manager.add_sub_track(TrackBuilder::default()).ok();
                Self {
                    manager: Some(manager),
                    bgm,
                    se,
                    pcm,
                    koe,
                    mov,
                    ..Self::default_volumes()
                }
            }
            Err(e) => {
                eprintln!("kira init failed, audio disabled: {:#}", e);
                Self::disabled("ASTRA_SIGLUS_NATIVE_AUDIO_INIT")
            }
        }
        #[cfg(not(feature = "native-audio"))]
        Self::disabled("ASTRA_SIGLUS_AUDIO_BACKEND_REQUIRED")
    }


    fn disabled(code: &'static str) -> Self {
        Self {
            manager: None,
            initialization_error: Some(code),
            bgm: None,
            se: None,
            pcm: None,
            koe: None,
            mov: None,
            ..Self::default_volumes()
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.manager.is_some()
    }

    pub fn check_initialization(&mut self) -> Result<()> {
        if let Some(code) = self.initialization_error { return Err(anyhow!(code)); }
        #[cfg(feature = "pausable-clock")]
        if let Some(manager) = self.manager.as_mut() { manager.backend_mut().check()?; }
        Ok(())
    }

    /// Stop the native device first, then cancel and join all decoder workers.
    pub fn shutdown(&mut self) -> Result<()> {
        #[cfg(feature = "pausable-clock")]
        let device = self.manager.as_mut().map_or(Ok(()), |manager| manager.backend_mut().shutdown().map_err(anyhow::Error::from));
        #[cfg(not(feature = "pausable-clock"))]
        let device: Result<()> = Ok(());
        drop(self.manager.take());
        #[cfg(not(target_arch = "wasm32"))]
        let decoders = self.decoders.stop_and_join().map_err(|_| anyhow!("ASTRA_SIGLUS_AUDIO_DECODER_PANIC"));
        #[cfg(target_arch = "wasm32")]
        let decoders: Result<()> = Ok(());
        device.and(decoders)
    }

    fn track_ref(&self, kind: TrackKind) -> Option<&TrackHandle> {
        match kind {
            TrackKind::Bgm => self.bgm.as_ref(),
            TrackKind::Se => self.se.as_ref(),
            TrackKind::Pcm => self.pcm.as_ref(),
            TrackKind::Koe => self.koe.as_ref(),
            TrackKind::Mov => self.mov.as_ref(),
        }
    }

    fn track_mut(&mut self, kind: TrackKind) -> Option<&mut TrackHandle> {
        match kind {
            TrackKind::Bgm => self.bgm.as_mut(),
            TrackKind::Se => self.se.as_mut(),
            TrackKind::Pcm => self.pcm.as_mut(),
            TrackKind::Koe => self.koe.as_mut(),
            TrackKind::Mov => self.mov.as_mut(),
        }
    }

    fn track_base_mut(&mut self, kind: TrackKind) -> &mut u8 {
        match kind {
            TrackKind::Bgm => &mut self.bgm_base,
            TrackKind::Se => &mut self.se_base,
            TrackKind::Pcm => &mut self.pcm_base,
            TrackKind::Koe => &mut self.koe_base,
            TrackKind::Mov => &mut self.mov_base,
        }
    }

    fn track_master_mut(&mut self, kind: TrackKind) -> &mut u8 {
        match kind {
            TrackKind::Bgm => &mut self.bgm_master,
            TrackKind::Se => &mut self.se_master,
            TrackKind::Pcm => &mut self.pcm_master,
            TrackKind::Koe => &mut self.koe_master,
            TrackKind::Mov => &mut self.mov_master,
        }
    }

    fn track_base(&self, kind: TrackKind) -> u8 {
        match kind {
            TrackKind::Bgm => self.bgm_base,
            TrackKind::Se => self.se_base,
            TrackKind::Pcm => self.pcm_base,
            TrackKind::Koe => self.koe_base,
            TrackKind::Mov => self.mov_base,
        }
    }

    fn track_master(&self, kind: TrackKind) -> u8 {
        match kind {
            TrackKind::Bgm => self.bgm_master,
            TrackKind::Se => self.se_master,
            TrackKind::Pcm => self.pcm_master,
            TrackKind::Koe => self.koe_master,
            TrackKind::Mov => self.mov_master,
        }
    }

    fn apply_track_volume(&mut self, kind: TrackKind) {
        let base = self.track_base(kind) as f64 / 255.0;
        let master = self.track_master(kind) as f64 / 255.0;
        let amp = base * master;
        let Some(track) = self.track_mut(kind) else {
            return;
        };
        track.set_volume(Volume::Amplitude(amp), Tween::default());
    }

    fn apply_track_volume_fade(&mut self, kind: TrackKind, fade_ms: i64) {
        let base = self.track_base(kind) as f64 / 255.0;
        let master = self.track_master(kind) as f64 / 255.0;
        let amp = base * master;
        let Some(track) = self.track_mut(kind) else {
            return;
        };
        let tween = if fade_ms > 0 {
            Tween {
                duration: std::time::Duration::from_millis(fade_ms as u64),
                ..Tween::default()
            }
        } else {
            Tween::default()
        };
        track.set_volume(Volume::Amplitude(amp), tween);
    }

    pub fn play_static(
        &mut self,
        kind: TrackKind,
        data: StaticSoundData,
    ) -> Result<StaticSoundHandle> {
        // Decide output destination before taking a mutable borrow of the manager.
        let data = if let Some(track) = self.track_ref(kind) {
            data.output_destination(track)
        } else {
            data
        };

        let manager = self
            .manager
            .as_mut()
            .ok_or_else(|| anyhow!("audio disabled"))?;

        manager.play(data).context("kira: play static sound")
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn play_streaming<E: Send + 'static>(
        &mut self,
        kind: TrackKind,
        data: StreamingSoundData<E>,
    ) -> Result<StreamingSoundHandle<E>> {
        let data = if let Some(track) = self.track_ref(kind) {
            data.output_destination(track)
        } else {
            data
        };
        let manager = self
            .manager
            .as_mut()
            .ok_or_else(|| anyhow!("audio disabled"))?;
        manager
            .play(data.with_decoder_owner(&mut self.decoders))
            .map_err(|_| anyhow!("kira: play streaming sound failed"))
    }

    pub fn set_track_volume_raw(&mut self, kind: TrackKind, volume_raw: u8) {
        *self.track_base_mut(kind) = volume_raw;
        self.apply_track_volume(kind);
    }

    pub fn set_track_master_volume_raw(&mut self, kind: TrackKind, volume_raw: u8) {
        *self.track_master_mut(kind) = volume_raw;
        self.apply_track_volume(kind);
    }

    pub fn set_track_volume_raw_fade(&mut self, kind: TrackKind, volume_raw: u8, fade_ms: i64) {
        *self.track_base_mut(kind) = volume_raw;
        self.apply_track_volume_fade(kind, fade_ms);
    }
}
