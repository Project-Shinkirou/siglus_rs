//! Explicit isolated-headless output; native sessions retain the CPAL backend.
use anyhow::{Result, anyhow};
use kira::manager::backend::{Backend, Renderer, cpal::{CpalBackend, CpalBackendSettings}};
use roukaku_platform::{AudioOutputLane, AudioOutputRequest, AudioWakeRegistration};
use roukaku_platform_common::{NativeAudioProducer, NullAudioDevice};
use std::{
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread::{self, JoinHandle},
    time::Duration,
};

pub fn requested() -> bool {
    std::env::var("SHINKI_HEADLESS_ISOLATED_CHILD").as_deref() == Ok("1")
        && std::env::var("SHINKI_HEADLESS_AUDIO_DEVICE").as_deref() == Ok("nullaudiodevice")
}

pub struct Settings {
    native: CpalBackendSettings,
    null: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self { native: CpalBackendSettings::default(), null: requested() }
    }
}

pub enum DesktopBackend {
    Native(CpalBackend),
    Null(NullBackend),
}
impl Backend for DesktopBackend {
    type Settings = Settings;
    type Error = anyhow::Error;

    fn setup(settings: Settings) -> Result<(Self, u32)> {
        if settings.null {
            let backend = NullBackend::open()?;
            eprintln!("SHINKI_HEADLESS_AUDIO_DEVICE=NullAudioDevice rate=48000 channels=2");
            Ok((Self::Null(backend), 48_000))
        } else {
            let (backend, rate) = CpalBackend::setup(settings.native)?;
            Ok((Self::Native(backend), rate))
        }
    }

    fn start(&mut self, renderer: Renderer) -> Result<()> {
        match self {
            Self::Native(backend) => backend.start(renderer).map_err(Into::into),
            Self::Null(backend) => backend.start(renderer),
        }
    }
}
impl DesktopBackend {
    #[cfg(feature = "pausable-clock")]
    pub fn set_suspended(&mut self, value: bool) {
        match self {
            Self::Native(backend) => backend.set_suspended(value),
            Self::Null(backend) => {
                backend.suspended.store(value, Ordering::Release);
                backend.wake.notify();
            }
        }
    }

    #[cfg(feature = "pausable-clock")]
    pub fn suspension_state(&self) -> Arc<AtomicBool> {
        match self {
            Self::Native(backend) => backend.suspension_state(),
            Self::Null(backend) => backend.suspended.clone(),
        }
    }

    #[cfg(feature = "pausable-clock")]
    pub fn check(&mut self) -> Result<()> {
        match self {
            Self::Native(backend) => backend.check().map_err(Into::into),
            Self::Null(backend) => {
                if backend.worker.as_ref().is_some_and(|worker| worker.is_finished()) {
                    backend.shutdown()?;
                    return Err(anyhow!("headless audio worker stopped unexpectedly"));
                }
                Ok(())
            }
        }
    }

    #[cfg(feature = "pausable-clock")]
    pub fn shutdown(&mut self) -> Result<()> {
        match self {
            Self::Native(backend) => backend.shutdown().map_err(Into::into),
            Self::Null(backend) => backend.shutdown(),
        }
    }
}

pub struct NullBackend {
    endpoint: Option<(NullAudioDevice, NativeAudioProducer)>,
    worker: Option<JoinHandle<Result<()>>>,
    stop: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
    wake: AudioWakeRegistration,
}
impl NullBackend {
    fn open() -> Result<Self> {
        let wake = AudioWakeRegistration::default();
        let endpoint = NullAudioDevice::open(AudioOutputRequest {
            sample_rate: 48_000,
            channels: 2,
            chunk_frames: 512,
            max_buffered_frames: 2048,
            start_paused: true,
            capture_samples: false,
        }, wake.clone())?;
        Ok(Self {
            endpoint: Some(endpoint),
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
            suspended: Arc::new(AtomicBool::new(false)),
            wake,
        })
    }

    fn start(&mut self, renderer: Renderer) -> Result<()> {
        let (device, producer) = self.endpoint.take().ok_or_else(|| anyhow!("headless audio already started"))?;
        let stop = self.stop.clone();
        let suspended = self.suspended.clone();
        let wake = self.wake.clone();
        self.worker = Some(thread::Builder::new().name("siglus-headless-mixer".into())
            .spawn(move || render(renderer, device, producer, stop, suspended, wake))?);
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        self.wake.notify();
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| anyhow!("headless audio worker panicked"))??;
        }
        self.endpoint.take();
        Ok(())
    }
}
impl Drop for NullBackend {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn render(
    mut renderer: Renderer,
    device: NullAudioDevice,
    mut producer: NativeAudioProducer,
    stop: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
    wake: AudioWakeRegistration,
) -> Result<()> {
    let mut samples = vec![0.0; 1024];
    let mut paused = true;
    let mut observed = 0;
    while !stop.load(Ordering::Acquire) {
        if suspended.load(Ordering::Acquire) {
            if !paused {
                device.pause()?;
                paused = true;
            }
            if let Some(next) = wake.wait_timeout(observed, Duration::from_millis(20)) {
                observed = next;
            }
            continue;
        }
        producer.wait_for_capacity(samples.len(), &stop)?;
        if stop.load(Ordering::Acquire) { break; }
        if suspended.load(Ordering::Acquire) { continue; }
        renderer.on_start_processing();
        for frame in samples.chunks_exact_mut(2) {
            let output = renderer.process();
            frame[0] = output.left;
            frame[1] = output.right;
        }
        samples = producer.submit(samples)?;
        if paused {
            device.resume()?;
            paused = false;
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "pausable-clock"))]
mod tests {
    use super::*;
    use kira::{Frame, manager::{AudioManager, AudioManagerSettings}, sound::static_sound::StaticSoundData};
    use std::time::Instant;

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "audio clock did not advance");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn null_mixer_advances_pauses_resumes_and_joins() {
        let mut manager = AudioManager::<DesktopBackend>::new(AudioManagerSettings {
            backend_settings: Settings { native: CpalBackendSettings::default(), null: true },
            ..Default::default()
        }).unwrap();
        let sound = StaticSoundData {
            sample_rate: 48_000,
            frames: vec![Frame { left: 0.25, right: -0.25 }; 480_000].into(),
            settings: Default::default(),
            slice: None,
        };
        let handle = manager.play(sound).unwrap();
        wait_until(|| handle.position() >= 0.05);
        manager.backend_mut().set_suspended(true);
        thread::sleep(Duration::from_millis(60));
        let paused = handle.position();
        thread::sleep(Duration::from_millis(60));
        assert_eq!(handle.position(), paused);
        manager.backend_mut().set_suspended(false);
        wait_until(|| handle.position() > paused + 0.02);
        manager.backend_mut().check().unwrap();
        manager.backend_mut().shutdown().unwrap();
        let closed = handle.position();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(handle.position(), closed);
        manager.backend_mut().shutdown().unwrap();
    }
}
