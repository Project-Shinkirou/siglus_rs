//! Game-initiated exit through the embedded host, using generated public scenes only.
#[path = "support/scene_fixture.rs"]
mod scene_fixture;

use scene_fixture::{Code, gameexe, scene_chunk, scene_pack};
use siglus_scene_vm::{
    host::{SiglusHost, SiglusHostConfig},
    render::Renderer,
    runtime::{
        forms::codes::{ELM_GLOBAL_OWARI, ELM_GLOBAL_SYSCOM, syscom_op},
        native_ui::{NativeMessageBoxRequest, NativeUiBackend},
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const END_GAME: [i32; 2] = [ELM_GLOBAL_SYSCOM, syscom_op::END_GAME];

struct Project {
    base: PathBuf,
}

impl Project {
    fn new(start: Vec<u8>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "siglus-public-host-exit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let project = Self { base };
        fs::create_dir_all(project.game()).unwrap();
        let menu = Code::default().line(1).eof();
        fs::write(
            project.game().join("Scene.pck"),
            scene_pack(&[
                ("start", scene_chunk(&start, &[])),
                ("menu", scene_chunk(&menu, &[])),
            ]),
        )
        .unwrap();
        fs::write(project.game().join("Gameexe.dat"), gameexe(1280, "menu")).unwrap();
        project
    }

    fn game(&self) -> PathBuf {
        self.base.join("game")
    }

    /// The original engine's save directory inside the game.
    fn saves(&self) -> PathBuf {
        self.game().join("savedata")
    }

    fn host(&self) -> SiglusHost {
        let renderer = pollster::block_on(Renderer::new_offscreen(1280, 720)).unwrap();
        let mut config = SiglusHostConfig::new(self.game());
        config.width = Some(1280);
        config.height = Some(720);
        SiglusHost::new_with_renderer_sync(config, renderer).unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn has_files(dir: &Path) -> bool {
    fs::read_dir(dir).unwrap().flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            has_files(&path)
        } else {
            true
        }
    })
}

/// Steps until the host reports the game ended itself, bounded so a missing
/// exit fails instead of hanging.
fn step_until_finished(host: &mut SiglusHost) {
    assert!(
        (0..240).any(|_| host.step(0).unwrap()),
        "game-initiated exit must finish the host"
    );
}

fn assert_finished_at_line(host: &mut SiglusHost, line: i32) {
    assert_eq!(host.vm_mut().current_scene_name(), Some("start"));
    assert_eq!(host.vm_mut().current_line_no(), line);
    assert!(!host.vm_mut().ctx.globals.system.active_flag);
    // Finished is sticky: later frames neither resume the script nor fail.
    for _ in 0..8 {
        assert!(host.step(0).unwrap());
    }
    assert_eq!(host.vm_mut().current_line_no(), line);
}

#[test]
#[ignore = "requires a GPU adapter"]
fn owari_finishes_the_host_without_resuming_the_script() {
    // Line 2 would run if EndGame fell back into the suspended script proc.
    let start = Code::default()
        .line(1)
        .command(&[ELM_GLOBAL_OWARI], &[])
        .line(2)
        .eof();
    let project = Project::new(start);
    let mut host = project.host();
    step_until_finished(&mut host);
    assert_finished_at_line(&mut host, 1);
    // EndGame persists global data before reporting the end.
    assert!(has_files(&project.saves()));
}

#[test]
#[ignore = "requires a GPU adapter"]
fn syscom_end_game_without_warning_finishes_the_host() {
    let start = Code::default()
        .line(1)
        .command(&END_GAME, &[0, 0, 0])
        .line(2)
        .eof();
    let project = Project::new(start);
    let mut host = project.host();
    step_until_finished(&mut host);
    assert_finished_at_line(&mut host, 1);
}

#[derive(Default)]
struct Prompts(Mutex<Vec<NativeMessageBoxRequest>>);

impl NativeUiBackend for Prompts {
    fn show_system_messagebox(&self, request: NativeMessageBoxRequest) {
        self.0.lock().unwrap().push(request);
    }
}

/// `END_GAME(warning)` on line 1, `OWARI` on line 2: the answer decides which
/// line ends the game. The project must outlive the host.
fn answer_end_game_warning(answer: i64) -> (SiglusHost, Project) {
    let start = Code::default()
        .line(1)
        .command(&END_GAME, &[1, 0, 0])
        .line(2)
        .command(&[ELM_GLOBAL_OWARI], &[])
        .line(3)
        .eof();
    let project = Project::new(start);
    let mut host = project.host();
    let prompts = Arc::new(Prompts::default());
    host.vm_mut()
        .ctx
        .set_native_ui_backend(Some(prompts.clone() as Arc<dyn NativeUiBackend>));
    let mut request = None;
    for _ in 0..240 {
        assert!(
            !host.step(0).unwrap(),
            "the warning must wait for an answer"
        );
        request = prompts.0.lock().unwrap().pop();
        if request.is_some() {
            break;
        }
    }
    let request = request.expect("END_GAME warning asks the host");
    assert!(request.buttons.iter().any(|b| b.value == answer));
    for _ in 0..8 {
        assert!(!host.step(0).unwrap(), "no answer means no exit");
    }
    host.submit_native_messagebox_result(request.request_id, answer);
    step_until_finished(&mut host);
    (host, project)
}

#[test]
#[ignore = "requires a GPU adapter"]
fn accepted_end_game_warning_finishes_the_host() {
    let (mut host, _project) = answer_end_game_warning(0);
    assert_finished_at_line(&mut host, 1);
}

#[test]
#[ignore = "requires a GPU adapter"]
fn declined_end_game_warning_resumes_the_script() {
    let (mut host, _project) = answer_end_game_warning(1);
    assert_finished_at_line(&mut host, 2);
}
