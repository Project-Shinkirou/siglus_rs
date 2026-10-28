use super::{
    ACTIVE, Failure, Result, abi, beta_saves, boundary, configuration, frames, initialized, input,
    platform,
};
use siglus_scene_vm::{
    host::{SiglusHost, SiglusHostConfig, default_frame_interval_ms},
    render::Renderer,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const INPUT_CAPACITY: usize = 256;
static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(1);
static LIVE: Mutex<Option<(usize, Arc<Session>)>> = Mutex::new(None);
enum Command {
    Input(input::Event),
    Pause(bool),
}
struct Mailbox {
    commands: VecDeque<Command>,
    frame: Option<frames::Published>,
    state: u32,
    failure: Option<Failure>,
}
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    cancelled: Arc<AtomicBool>,
    context: Arc<AtomicU64>,
    epoch: u64,
    platform: platform::Service,
    host_cancel: platform::Cancellation,
    cancel_callbacks_closed: Mutex<bool>,
}
struct Session {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<Result<()>>>>,
    info: abi::SurfaceInfo,
}
impl Shared {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let closed = self
            .cancel_callbacks_closed
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !*closed {
            self.platform
                .cancel(self.epoch, self.context.load(Ordering::Acquire));
        }
        self.wake.notify_all();
    }
    fn fail(&self, failure: Failure) {
        let mut m = self.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        m.failure = Some(failure);
        m.state = abi::SESSION_FAILED;
        self.wake.notify_all();
    }
    fn enqueue(&self, commands: impl IntoIterator<Item = Command>, count: usize) -> Result<()> {
        let mut m = self.mailbox.lock().map_err(|_| abi::PANICKED)?;
        if let Some(failure) = &m.failure {
            return Err(failure.clone());
        }
        if self.cancelled.load(Ordering::Acquire) || m.state == abi::SESSION_FINISHED {
            return Err(abi::INVALID_STATE.into());
        }
        if m.commands.len() + count > INPUT_CAPACITY {
            return Err(abi::CAPACITY.into());
        }
        m.commands.extend(commands);
        self.wake.notify_one();
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.shared.cancel();
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}
fn session(handle: abi::Session) -> Result<Arc<Session>> {
    let live = LIVE.lock().map_err(|_| abi::PANICKED)?;
    live.as_ref()
        .filter(|(id, _)| *id == handle as usize)
        .map(|(_, session)| Arc::clone(session))
        .ok_or_else(|| abi::INVALID_ARGUMENT.into())
}
pub(super) unsafe extern "C" fn open(
    request: *const abi::OpenRequest,
    out: *mut abi::OpenResult,
    error: *mut abi::Error,
) -> abi::Status {
    if !out.is_null() {
        unsafe {
            (*out).session = std::ptr::null_mut();
        }
    }
    boundary(error, || {
        initialized()?;
        let request = unsafe { request.as_ref() }.ok_or(abi::INVALID_ARGUMENT)?;
        let out = unsafe { out.as_mut() }.ok_or(abi::INVALID_ARGUMENT)?;
        if !request.header.accepts::<abi::OpenRequest>()
            || !request.host.header.accepts::<abi::HostServices>()
            || request.session_epoch == 0
            || !input::valid_window(request.window)
        {
            return Err(abi::INVALID_ARGUMENT.into());
        }
        abi::RenderExtentSupport::default()
            .validate_request(abi::CAP_CPU_SURFACE, request.render_extent)?;
        let config = unsafe {
            abi::borrowed_array(
                request.configuration,
                request.configuration_count,
                abi::MAX_CONFIG_FIELDS,
            )?
        };
        let options = unsafe { configuration::parse(config) }?;
        let path = PathBuf::from(unsafe { request.game_path.text(32768)? });
        if !path.is_dir() {
            return Err(abi::INVALID_ARGUMENT.into());
        }
        let service = platform::Service::new(request.host.window)?;
        let host_cancel = platform::Cancellation::new(request.host.cancellation)?;
        host_cancel.check(request.session_epoch)?;
        if ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(abi::INVALID_STATE.into());
        }
        struct Opening;
        impl Drop for Opening {
            fn drop(&mut self) {
                ACTIVE.store(false, Ordering::Release);
            }
        }
        let opening = Opening;
        let id = NEXT_HANDLE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| abi::CAPACITY)?;
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox {
                commands: VecDeque::with_capacity(INPUT_CAPACITY),
                frame: None,
                state: abi::SESSION_WAITING,
                failure: None,
            }),
            wake: Condvar::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            context: Arc::new(AtomicU64::new(1)),
            epoch: request.session_epoch,
            platform: service,
            host_cancel,
            cancel_callbacks_closed: Mutex::new(false),
        });
        let owner = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let window = request.window;
        let worker = thread::Builder::new()
            .name("siglus-native".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&owner, path, options, window, ready_tx)
                }))
                .unwrap_or_else(|_| {
                    Err(Failure::new(
                        abi::PANICKED,
                        "SHINKI_EMU_SIGLUS_PANIC",
                        "native worker panicked",
                    ))
                });
                if let Err(failure) = &result {
                    owner.fail(failure.clone());
                }
                result
            })
            .map_err(Failure::native)?;
        let info = match ready_rx.recv() {
            Ok(info) => info,
            Err(_) => {
                shared.cancel();
                let result = worker.join();
                return Err(match result {
                    Ok(Err(failure)) => failure,
                    _ => Failure::new(
                        abi::CORE_ERROR,
                        "SHINKI_EMU_SIGLUS_OPEN",
                        "native worker stopped before initialization",
                    ),
                });
            }
        };
        if let Err(failure) = host_cancel.check(request.session_epoch) {
            shared.cancel();
            let _ = worker.join();
            return Err(failure);
        }
        let session = Arc::new(Session {
            shared,
            worker: Mutex::new(Some(worker)),
            info,
        });
        *LIVE.lock().map_err(|_| abi::PANICKED)? = Some((id, session));
        std::mem::forget(opening);
        *out = abi::OpenResult {
            session: id as abi::Session,
            context_epoch: 1,
            output_epoch: 1,
            surface: info,
        };
        Ok(())
    })
}
fn run(
    shared: &Shared,
    path: PathBuf,
    options: configuration::EmbeddedConfig,
    window: abi::WindowState,
    ready: mpsc::SyncSender<abi::SurfaceInfo>,
) -> Result<()> {
    shared.host_cancel.check(shared.epoch)?;
    beta_saves::import(&path).map_err(Failure::native)?;
    let resource_language = options.resource_language;
    let (width, height) = configuration::screen_size(&path, resource_language)?;
    shared.host_cancel.check(shared.epoch)?;
    let renderer =
        pollster::block_on(Renderer::new_offscreen(width, height)).map_err(Failure::native)?;
    shared.host_cancel.check(shared.epoch)?;
    let mut config = SiglusHostConfig::new(path);
    config.resource_language = resource_language;
    config.width = Some(width);
    config.height = Some(height);
    let mut host = SiglusHost::new_with_renderer_sync(config, renderer).map_err(Failure::native)?;
    shared.host_cancel.check(shared.epoch)?;
    host.vm_mut()
        .ctx
        .audio
        .check_initialization()
        .map_err(Failure::native)?;
    let logical = host.logical_size();
    let info = configuration::surface(width, height, logical)?;
    let platform = Arc::new(platform::Platform {
        service: shared.platform,
        epoch: shared.epoch,
        context: Arc::clone(&shared.context),
        cancelled: Arc::clone(&shared.cancelled),
        host_cancel: shared.host_cancel,
        audio_paused: Mutex::new(
            host.vm_mut()
                .ctx
                .audio
                .suspension_state()
                .map_err(Failure::native)?,
        ),
        completion: Mutex::new(None),
    });
    host.vm_mut()
        .ctx
        .set_native_ui_backend(Some(platform.clone()));
    let fingerprint = host.persist_global_if_changed(None);
    let mut readback = frames::Readback::new();
    let result = drive(
        shared,
        &mut host,
        &platform,
        &mut readback,
        window,
        info,
        ready,
    );
    // Cleanup also runs on normal engine errors. Panic unwinding drops the native
    // renderer/mixer on this owner thread; no host context outlives join.
    readback.cancel(&mut host.renderer_mut());
    let audio = host.vm_mut().ctx.audio.shutdown().map_err(Failure::native);
    let voice = host.vm_mut().ctx.koe.shutdown().map_err(Failure::native);
    let movie = host.vm_mut().ctx.movie.shutdown().map_err(Failure::native);
    siglus_scene_vm::platform_time::set_paused(false);
    host.persist_global_if_changed(Some(fingerprint));
    result.and(audio).and(voice).and(movie)
}
fn drive(
    shared: &Shared,
    host: &mut SiglusHost,
    platform: &platform::Platform,
    readback: &mut frames::Readback,
    mut window: abi::WindowState,
    info: abi::SurfaceInfo,
    ready: mpsc::SyncSender<abi::SurfaceInfo>,
) -> Result<()> {
    let logical = host.logical_size();
    let started = Instant::now();
    let mut deadline = started;
    let mut explicitly_paused = false;
    let mut suspended = window.suspended != 0 || window.focused == 0;
    host.set_suspended(suspended);
    {
        shared.mailbox.lock().map_err(|_| abi::PANICKED)?.state = if suspended {
            abi::SESSION_PAUSED
        } else {
            abi::SESSION_RUNNING
        };
    }
    shared.host_cancel.check(shared.epoch)?;
    ready.send(info).map_err(Failure::native)?;
    let mut finished = false;
    let mut first_frame = true;
    loop {
        if shared.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        shared.host_cancel.check(shared.epoch)?;
        loop {
            let command = shared
                .mailbox
                .lock()
                .map_err(|_| abi::PANICKED)?
                .commands
                .pop_front();
            let Some(command) = command else {
                break;
            };
            match command {
                Command::Pause(paused) => explicitly_paused = paused,
                Command::Input(input::Event::Window(value)) => window = value,
                Command::Input(event) => {
                    if !suspended && !finished {
                        event.apply(host);
                    }
                }
            }
            let next = explicitly_paused || window.suspended != 0 || window.focused == 0;
            if next != suspended {
                suspended = next;
                host.set_suspended(next);
                deadline = Instant::now();
            }
        }
        if (!suspended || first_frame) && !finished && Instant::now() >= deadline {
            finished = host.step(0).map_err(Failure::native)?;
            first_frame = false;
            if let Some(result) = platform
                .completion
                .lock()
                .map_err(|_| abi::PANICKED)?
                .take()
            {
                let (id, value) = result?;
                host.submit_native_messagebox_result(id, value);
            }
            if host.take_timeline_reset() {
                let old = shared.context.fetch_add(1, Ordering::AcqRel);
                shared.platform.cancel(shared.epoch, old);
                readback.cancel(&mut host.renderer_mut());
                *platform.audio_paused.lock().map_err(|_| abi::PANICKED)? = host
                    .vm_mut()
                    .ctx
                    .audio
                    .suspension_state()
                    .map_err(Failure::native)?;
                host.vm_mut()
                    .ctx
                    .audio
                    .check_initialization()
                    .map_err(Failure::native)?;
            }
            let interval = if host.vm_mut().ctx.globals.script.wait_display_vsync_off_flag {
                Duration::ZERO
            } else {
                Duration::from_millis(u64::from(default_frame_interval_ms(0)))
            };
            deadline = Instant::now() + interval;
        }
        if let Some(frame) = readback.poll(
            &mut host.renderer_mut(),
            shared.epoch,
            logical,
            started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        )? {
            shared.mailbox.lock().map_err(|_| abi::PANICKED)?.frame = Some(frame);
        }
        let mut mailbox = shared.mailbox.lock().map_err(|_| abi::PANICKED)?;
        mailbox.state = if finished {
            abi::SESSION_FINISHED
        } else if suspended {
            abi::SESSION_PAUSED
        } else {
            abi::SESSION_RUNNING
        };
        if !mailbox.commands.is_empty() {
            continue;
        }
        let wait = if suspended || finished {
            Duration::from_millis(1)
        } else {
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(1))
        };
        let _ = shared
            .wake
            .wait_timeout(mailbox, wait)
            .map_err(|_| abi::PANICKED)?;
    }
}
pub(super) unsafe extern "C" fn input(
    handle: abi::Session,
    events: *const abi::InputEvent,
    count: usize,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        let session = session(handle)?;
        let events = unsafe { abi::borrowed_array(events, count, INPUT_CAPACITY)? };
        let mut mailbox = session.shared.mailbox.lock().map_err(|_| abi::PANICKED)?;
        if let Some(failure) = &mailbox.failure {
            return Err(failure.clone());
        }
        if session.shared.cancelled.load(Ordering::Acquire)
            || mailbox.state == abi::SESSION_FINISHED
        {
            return Err(abi::INVALID_STATE.into());
        }
        if mailbox.state == abi::SESSION_PAUSED
            && events.iter().any(|e| e.kind != abi::INPUT_WINDOW)
        {
            return Err(abi::INVALID_STATE.into());
        }
        let generation = mailbox.frame.as_ref().map_or(0, |f| f.identity.generation);
        let previous = mailbox.commands.len();
        if previous + count > INPUT_CAPACITY {
            return Err(abi::CAPACITY.into());
        }
        for event in events {
            match unsafe { input::decode(event, session.shared.epoch, generation) } {
                Ok(event) => mailbox.commands.push_back(Command::Input(event)),
                Err(failure) => {
                    mailbox.commands.truncate(previous);
                    return Err(failure);
                }
            }
        }
        session.shared.wake.notify_one();
        Ok(())
    })
}
pub(super) unsafe extern "C" fn control(
    handle: abi::Session,
    control: u32,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        let session = session(handle)?;
        let paused = match control {
            abi::CONTROL_PAUSE => true,
            abi::CONTROL_RESUME => false,
            _ => return Err(abi::UNSUPPORTED.into()),
        };
        session.shared.enqueue([Command::Pause(paused)], 1)
    })
}
pub(super) unsafe extern "C" fn state(
    handle: abi::Session,
    out: *mut abi::SessionState,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        let session = session(handle)?;
        let out = unsafe { out.as_mut() }.ok_or(abi::INVALID_ARGUMENT)?;
        let mailbox = session.shared.mailbox.lock().map_err(|_| abi::PANICKED)?;
        *out = abi::SessionState {
            state: mailbox.state,
            flags: 0,
            context_epoch: session.shared.context.load(Ordering::Acquire),
            output_epoch: 1,
            error: mailbox
                .failure
                .as_ref()
                .map_or_else(abi::Error::default, Failure::error),
        };
        Ok(())
    })
}
pub(super) unsafe extern "C" fn frame(
    handle: abi::Session,
    after: abi::FrameIdentity,
    out: *mut abi::FrameSurface,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        let session = session(handle)?;
        let out = unsafe { out.as_mut() }.ok_or(abi::INVALID_ARGUMENT)?;
        let mailbox = session.shared.mailbox.lock().map_err(|_| abi::PANICKED)?;
        if let Some(failure) = &mailbox.failure {
            return Err(failure.clone());
        }
        *out = match &mailbox.frame {
            Some(frame) => frame.surface(after),
            None => abi::FrameSurface {
                state: abi::FRAME_PENDING,
                reserved: 0,
                identity: abi::FrameIdentity {
                    session_epoch: session.shared.epoch,
                    output_epoch: 1,
                    generation: 0,
                },
                timestamp_ns: 0,
                info: session.info,
                cpu: frames::empty_lease(),
            },
        };
        Ok(())
    })
}
pub(super) unsafe extern "C" fn cancel(handle: abi::Session) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Ok(session) = session(handle) {
            session.shared.cancel();
        }
    }));
}
pub(super) unsafe extern "C" fn close(handle: abi::Session, error: *mut abi::Error) -> abi::Status {
    boundary(error, || {
        let session = {
            let mut live = LIVE.lock().unwrap_or_else(|p| p.into_inner());
            if !live.as_ref().is_some_and(|(id, _)| *id == handle as usize) {
                return Err(abi::INVALID_ARGUMENT.into());
            }
            live.take().expect("matched active session").1
        };
        struct Closing;
        impl Drop for Closing {
            fn drop(&mut self) {
                ACTIVE.store(false, Ordering::Release);
            }
        }
        let _closing = Closing;
        session.shared.cancel();
        *session
            .shared
            .cancel_callbacks_closed
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = true;
        let worker = session
            .worker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(worker) = worker {
            worker.join().map_err(|_| {
                Failure::new(
                    abi::PANICKED,
                    "SHINKI_EMU_SIGLUS_PANIC",
                    "native worker panicked during cleanup",
                )
            })??;
        }
        Ok(())
    })
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
