use std::io::Write;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use slint::{Timer, TimerMode};
use slint_backend_kindle::WakeSchedule;

slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../../LiberationSans-Regular.ttf");

const CAT_FACTS_URL: &str = "https://catfact.ninja/fact";
const MAX_FETCH_ATTEMPTS: u32 = 6;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_LOG_LINES: usize = 12;

// Per-run log directory. Each launch gets its own timestamped file so
// consecutive experiments don't blur together. The launcher script sets
// RUN_TS to the same stamp it uses for its own stderr capture, so the two
// files land next to each other and pair cleanly in analyze-logs.py.
const LOG_DIR: &str = "/mnt/us/slint-kindle-logs";

#[derive(serde::Deserialize)]
struct CatFact {
    fact: String,
}

struct Logger {
    lines: Vec<String>,
    file_path: String,
}

impl Logger {
    fn new(file_path: String) -> Self {
        if let Some(parent) = std::path::Path::new(&file_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self {
            lines: Vec::new(),
            file_path,
        }
    }

    fn push(&mut self, line: impl Into<String>) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        let formatted = format!("[{stamp}] {}", line.into());
        // Best-effort file append; on the dev host the directory doesn't
        // exist and we just silently skip — the on-screen view still works.
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
        {
            let _ = writeln!(f, "{formatted}");
        }
        self.lines.push(formatted);
        while self.lines.len() > MAX_LOG_LINES {
            self.lines.remove(0);
        }
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Build the path for this run's structured log. The launcher exports
/// `RUN_TS=YYYYMMDDTHHMMSS`; when run directly without the launcher we fall
/// back to computing our own stamp so files still get unique names.
fn log_file_path() -> String {
    let ts = std::env::var("RUN_TS").unwrap_or_else(|_| {
        chrono::Local::now().format("%Y%m%dT%H%M%S").to_string()
    });
    format!("{LOG_DIR}/{ts}-cat-facts.log")
}

/// Report the first wireless/ethernet interface's link state. Wifi typically
/// transitions `down` → `up` over 5–15 s after a resume; a probe in the log
/// helps explain why early fetch attempts may fail.
fn link_state() -> String {
    for iface in ["wlan0", "wlan1", "eth0"] {
        let path = format!("/sys/class/net/{iface}/operstate");
        if let Ok(state) = std::fs::read_to_string(&path) {
            return format!("{iface}={}", state.trim());
        }
    }
    "no-iface".to_string()
}

/// Read the current RTC wakealarm value. Empty / "0" means no alarm set.
/// A future epoch second means *someone* has armed it — either us, just
/// before suspend, or powerd as part of its own state machine.
fn wakealarm_value() -> String {
    for n in 0..4 {
        let path = format!("/sys/class/rtc/rtc{n}/wakealarm");
        if let Ok(v) = std::fs::read_to_string(&path) {
            let trimmed = v.trim();
            return if trimmed.is_empty() {
                format!("rtc{n}=empty")
            } else {
                format!("rtc{n}={trimmed}")
            };
        }
    }
    "no-rtc".to_string()
}

fn fetch_cat_fact() -> Result<String, String> {
    let response = ureq::get(CAT_FACTS_URL)
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|e| format!("{e}"))?;
    let body: CatFact = response.into_json().map_err(|e| format!("parse: {e}"))?;
    Ok(body.fact)
}

fn push_log(log: &Arc<Mutex<Logger>>, weak: &slint::Weak<AppWindow>, line: String) {
    let text = {
        let mut logger = log.lock().expect("log poisoned");
        logger.push(line);
        logger.text()
    };
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.set_log_text(text.into());
        }
    });
}

fn set_fact(weak: &slint::Weak<AppWindow>, fact: String) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.set_fact_text(fact.into());
        }
    });
}

/// Run the fetch on a background thread so the UI stays responsive while we
/// wait for wifi to reconnect and retry. Each attempt logs the current link
/// state — that's the signal you're looking for when verifying the wake path.
fn spawn_fetch(log: Arc<Mutex<Logger>>, weak: slint::Weak<AppWindow>) {
    std::thread::spawn(move || {
        for attempt in 1..=MAX_FETCH_ATTEMPTS {
            push_log(
                &log,
                &weak,
                format!("fetch #{attempt} ({})", link_state()),
            );
            match fetch_cat_fact() {
                Ok(fact) => {
                    push_log(&log, &weak, format!("ok: {} chars", fact.len()));
                    set_fact(&weak, fact);
                    return;
                }
                Err(e) => {
                    let trimmed: String = e.chars().take(50).collect();
                    push_log(&log, &weak, format!("err: {trimmed}"));
                    if attempt < MAX_FETCH_ATTEMPTS {
                        std::thread::sleep(RETRY_DELAY);
                    }
                }
            }
        }
        push_log(&log, &weak, "giving up".into());
    });
}

fn main() {
    let backend =
        slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    let log = Arc::new(Mutex::new(Logger::new(log_file_path())));
    push_log(&log, &app.as_weak(), "startup".into());

    let tick = {
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let now = chrono::Local::now();
            app.set_time_text(now.format("%H:%M:%S").to_string().into());
        }
    };
    tick();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), tick);

    spawn_fetch(log.clone(), app.as_weak());


    let backend = backend.set_wake_schedule(WakeSchedule {
        wake_interval: Duration::from_secs(3600),
        stay_awake: Duration::from_secs(30),
    });

    let last_wake = Arc::new(Mutex::new(None::<SystemTime>));
    let cycle = Arc::new(Mutex::new(0u32));
    let mid_window_probe = Rc::new(Timer::default());

    backend.on_wake({
        let log = log.clone();
        let weak = app.as_weak();
        let last_wake = last_wake.clone();
        let cycle = cycle.clone();
        let probe_timer = mid_window_probe.clone();
        let probe_log = log.clone();
        let probe_weak = app.as_weak();
        move || {
            let now = SystemTime::now();
            let delta = {
                let mut last = last_wake.lock().expect("last_wake poisoned");
                let d = last.and_then(|prev| now.duration_since(prev).ok());
                *last = Some(now);
                d
            };
            let cycle_n = {
                let mut c = cycle.lock().expect("cycle poisoned");
                *c += 1;
                *c
            };
            let delta_str = delta
                .map(|d| format!("Δ{}s", d.as_secs()))
                .unwrap_or_else(|| "first".to_string());

            let text = {
                let mut logger = log.lock().expect("log poisoned");
                logger.push(format!(
                    "wake #{cycle_n} {delta_str} {}",
                    wakealarm_value()
                ));
                logger.text()
            };
            if let Some(app) = weak.upgrade() {
                let now_local = chrono::Local::now();
                app.set_time_text(now_local.format("%H:%M:%S").to_string().into());
                app.set_log_text(text.into());
            }

            probe_timer.start(TimerMode::SingleShot, Duration::from_secs(15), {
                let log = probe_log.clone();
                let weak = probe_weak.clone();
                move || {
                    let text = {
                        let mut logger = log.lock().expect("log poisoned");
                        logger.push(format!("mid-window probe: {}", wakealarm_value()));
                        logger.text()
                    };
                    if let Some(app) = weak.upgrade() {
                        app.set_log_text(text.into());
                    }
                }
            });

            spawn_fetch(log.clone(), weak.clone());
        }
    });

    app.run().expect("event loop error");
}
