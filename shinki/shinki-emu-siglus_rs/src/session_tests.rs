//! A game that ends itself reaches the Manager as SESSION_FINISHED through the
//! real ABI table; the process stays alive and close releases the core.
use super::*;
use crate::INITIALIZED;

#[path = "../../../crates/siglus_scene_vm/tests/support/scene_fixture.rs"]
mod scene_fixture;

use scene_fixture::{Code, gameexe, scene_chunk, scene_pack};
use siglus_scene_vm::runtime::forms::codes::{ELM_GLOBAL_OWARI, ELM_GLOBAL_SYSCOM, syscom_op};
use std::{fs, sync::atomic::AtomicU32};

/// The core admits one live session per process; tests open them one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

struct Project {
    base: PathBuf,
}

impl Project {
    fn new(name: &str, start: Vec<u8>) -> Self {
        let base = std::env::temp_dir().join(format!(
            "siglus-public-adapter-exit-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("game")).unwrap();
        let menu = Code::default().line(1).eof();
        fs::write(
            base.join("game/Scene.pck"),
            scene_pack(&[
                ("start", scene_chunk(&start, &[])),
                ("menu", scene_chunk(&menu, &[])),
            ]),
        )
        .unwrap();
        fs::write(base.join("game/Gameexe.dat"), gameexe(1280, "menu")).unwrap();
        Self { base }
    }

    fn saves_written(&self) -> bool {
        fn any(dir: &std::path::Path) -> bool {
            fs::read_dir(dir).unwrap().flatten().any(|entry| {
                let path = entry.path();
                if path.is_dir() { any(&path) } else { true }
            })
        }
        let saves = self.base.join("game/savedata");
        saves.is_dir() && any(&saves)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

unsafe extern "C" fn never_cancelled(_: *mut std::ffi::c_void, _: u64) -> u32 {
    0
}

/// Window host that answers every modal with the YES button and counts calls.
unsafe extern "C" fn answer_yes(
    context: *mut std::ffi::c_void,
    request: *const abi::WindowRequest,
    result: *mut abi::WindowResult,
    _: *mut abi::Error,
) -> abi::Status {
    let request = unsafe { &*request };
    let buttons = unsafe { std::slice::from_raw_parts(request.buttons, request.button_count) };
    let yes = buttons
        .iter()
        .find(|b| unsafe { b.label.text(256) } == Ok("YES"))
        .expect("END_GAME warning offers YES");
    unsafe { &*(context as *const AtomicU32) }.fetch_add(1, Ordering::AcqRel);
    let result = unsafe { &mut *result };
    result.selected_button = yes.id;
    result.valid_fields = abi::WINDOW_RESULT_MODAL;
    assert_eq!(request.kind, abi::WINDOW_MODAL);
    abi::OK
}

unsafe extern "C" fn ignore_invalidate(_: *mut std::ffi::c_void, _: u64, _: u64) {}

fn open_session(project: &Project, modals: Option<&AtomicU32>) -> abi::Session {
    INITIALIZED.store(true, Ordering::Release);
    let game = project.base.join("game");
    let game = game.to_str().unwrap();
    let config: [abi::ConfigEntry; 0] = [];
    let window = match modals {
        Some(count) => abi::WindowService {
            context: count as *const AtomicU32 as *mut _,
            request: Some(answer_yes),
            invalidate: Some(ignore_invalidate),
        },
        None => abi::WindowService {
            context: std::ptr::null_mut(),
            request: None,
            invalidate: None,
        },
    };
    let request = abi::OpenRequest {
        header: abi::Header::of::<abi::OpenRequest>(),
        game_path: abi::Bytes::new(game.as_bytes()),
        session_epoch: 7,
        configuration: config.as_ptr(),
        configuration_count: config.len(),
        window: abi::WindowState {
            drawable_width: 1280,
            drawable_height: 720,
            scale_factor: 1.0,
            focused: 1,
            suspended: 0,
        },
        host: abi::HostServices {
            header: abi::Header::of::<abi::HostServices>(),
            text: abi::TextService {
                context: std::ptr::null_mut(),
                request: None,
                acknowledge: None,
                invalidate: None,
            },
            window,
            cancellation: abi::CancellationService {
                context: std::ptr::null_mut(),
                is_cancelled: Some(never_cancelled),
            },
        },
        module_owner: std::ptr::null_mut(),
        render_extent: abi::Extent::default(),
    };
    let mut out = std::mem::MaybeUninit::<abi::OpenResult>::zeroed();
    let mut error = abi::Error::default();
    assert_eq!(
        unsafe { open(&request, out.as_mut_ptr(), &mut error) },
        abi::OK,
        "{}",
        String::from_utf8_lossy(&error.message[..error.message_len as usize])
    );
    unsafe { out.assume_init() }.session
}

fn session_state(handle: abi::Session) -> abi::SessionState {
    let mut out = std::mem::MaybeUninit::<abi::SessionState>::zeroed();
    let mut error = abi::Error::default();
    assert_eq!(
        unsafe { state(handle, out.as_mut_ptr(), &mut error) },
        abi::OK
    );
    unsafe { out.assume_init() }
}

fn wait_finished(handle: abi::Session) -> abi::SessionState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let current = session_state(handle);
        assert_ne!(
            current.state,
            abi::SESSION_FAILED,
            "game exit is not a failure"
        );
        if current.state == abi::SESSION_FINISHED {
            return current;
        }
        assert!(
            Instant::now() < deadline,
            "game exit never finished the session"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn assert_finished_then_closed(handle: abi::Session) {
    let finished = wait_finished(handle);
    assert_eq!(finished.flags, 0);
    assert_eq!(finished.error.code_len, 0);
    // Finished is terminal: it does not fall back to running and refuses input/control.
    thread::sleep(Duration::from_millis(50));
    assert_eq!(session_state(handle).state, abi::SESSION_FINISHED);
    let mut error = abi::Error::default();
    assert_eq!(
        unsafe { control(handle, abi::CONTROL_PAUSE, &mut error) },
        abi::INVALID_STATE
    );
    assert_eq!(unsafe { close(handle, &mut error) }, abi::OK);
    assert!(!ACTIVE.load(Ordering::Acquire));
    assert!(LIVE.lock().unwrap().is_none());
}

#[test]
#[ignore = "requires a GPU adapter"]
fn script_owari_reports_finished_and_close_releases_the_core() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let start = Code::default()
        .line(1)
        .command(&[ELM_GLOBAL_OWARI], &[])
        .line(2)
        .eof();
    let project = Project::new("owari", start);
    let handle = open_session(&project, None);
    assert_finished_then_closed(handle);
    assert!(project.saves_written(), "EndGame persists global data");
    // The core remains usable in the same process after a game ended itself.
    let again = open_session(&project, None);
    assert_finished_then_closed(again);
}

#[test]
#[ignore = "requires a GPU adapter"]
fn confirmed_end_game_warning_reports_finished() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let start = Code::default()
        .line(1)
        .command(&[ELM_GLOBAL_SYSCOM, syscom_op::END_GAME], &[1, 0, 0])
        .line(2)
        .eof();
    let project = Project::new("end-game-warning", start);
    let modals = AtomicU32::new(0);
    let handle = open_session(&project, Some(&modals));
    assert_finished_then_closed(handle);
    assert_eq!(modals.load(Ordering::Acquire), 1);
}
