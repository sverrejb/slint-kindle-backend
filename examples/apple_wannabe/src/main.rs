use std::fmt::Write;
use std::time::Duration;

use slint::{SharedString, Timer, TimerMode};

mod font;

slint::include_modules!();

const WORDS: &[&str] = &["Hello!", "Hola!", "Bonjour!", "Hallo!", "Ciao!"];

static HERSHEY_FONT: &str = include_str!("../scripts.jhf");

#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
static LIBERATION_SANS: &[u8] = include_bytes!("../../LiberationSans-Regular.ttf");

#[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
fn install_backend() -> slint_backend_kindle::KindleBackend {
    let backend =
        slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    backend.set_black_and_white(true);
    backend
}

const TICK: Duration = Duration::from_millis(33);
const WORD_DURATION: Duration = Duration::from_secs(3); 
const PAUSE: Duration = Duration::from_secs(2); 
const BLANK: Duration = Duration::from_millis(700);

fn commands_for(points: &[(f32, f32, bool)], n: usize) -> SharedString {
    let mut s = String::with_capacity(n * 16);
    for &(x, y, pen_lift) in &points[..n] {
        let cmd = if pen_lift { 'M' } else { 'L' };
        let _ = write!(s, "{cmd} {x:.2} {y:.2} ");
    }
    s.into()
}

enum Phase {
    Writing,
    Holding(u32),
    Clearing(u32),
}

struct Scribe {
    index: usize,
    points: Vec<(f32, f32, bool)>,
    revealed: usize,
    stride: usize,
    phase: Phase,
    hold_ticks: u32,
    clear_ticks: u32,
}

impl Scribe {
    fn new() -> Self {
        let ticks = |d: Duration| (d.as_millis() / TICK.as_millis()) as u32;
        Self {
            index: 0,
            points: Vec::new(),
            revealed: 0,
            stride: 1,
            phase: Phase::Writing,
            hold_ticks: ticks(PAUSE),
            clear_ticks: ticks(BLANK),
        }
    }

    fn write_word(&mut self, app: &AppWindow, word: &str) {
        let (points, viewbox) = font::build_points(HERSHEY_FONT, word);
        app.set_vb_x(viewbox.0);
        app.set_vb_y(viewbox.1);
        app.set_vb_w(viewbox.2);
        app.set_vb_h(viewbox.3);
        app.set_commands(SharedString::new());

        self.stride = (points.len() * TICK.as_millis() as usize / WORD_DURATION.as_millis() as usize).max(1);
        self.points = points;
        self.revealed = 0;
        self.phase = Phase::Writing;
    }

    fn tick(&mut self, app: &AppWindow) {
        match self.phase {
            Phase::Writing => {
                let total = self.points.len();
                self.revealed = (self.revealed + self.stride).min(total);
                app.set_commands(commands_for(&self.points, self.revealed));
                if self.revealed == total {
                    self.phase = Phase::Holding(self.hold_ticks);
                }
            }
            Phase::Holding(0) => {
                app.set_commands(SharedString::new());
                self.phase = Phase::Clearing(self.clear_ticks);
            }
            Phase::Holding(remaining) => self.phase = Phase::Holding(remaining - 1),
            Phase::Clearing(0) => {
                self.index = (self.index + 1) % WORDS.len();
                self.write_word(app, WORDS[self.index]);
            }
            Phase::Clearing(remaining) => self.phase = Phase::Clearing(remaining - 1),
        }
    }
}

fn main() {
    #[cfg(all(target_arch = "arm", target_os = "linux", target_env = "musl"))]
    let _backend = install_backend();

    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    let mut scribe = Scribe::new();
    scribe.write_word(&app, WORDS[0]);

    let timer = Timer::default();
    let weak = app.as_weak();
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(app) = weak.upgrade() else { return };
        scribe.tick(&app);
    });

    app.run().expect("event loop error");
}
