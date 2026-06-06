use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode, VecModel};

slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../../LiberationSans-Regular.ttf");

// Once the finger has been still this long, the stroke starts erasing — this is
// the pause before the first segment vanishes.
const ERASE_DELAY: Duration = Duration::from_millis(500);
// Poll cadence for ageing the trail. E-ink tops out ~30 fps, so a coarse tick is
// plenty (and keeps the SoC cool); each tick removes every dab now due, so the
// total erase time stays accurate regardless of granularity.
const TICK: Duration = Duration::from_millis(80);

// The live trail: dab birth times (oldest first), kept parallel to the UI model.
// `erase_anchor` is the oldest dab's time captured when erasing begins, so the
// removals replay the gaps between births — i.e. the speed the stroke was drawn.
#[derive(Default)]
struct Trail {
    born: Vec<Instant>,
    erase_anchor: Option<Instant>,
    // Previous sampled point of the current stroke, so we can join it to the
    // next one. `None` between strokes (reset on lift) so we don't draw a line
    // connecting two separate strokes.
    last_point: Option<(f32, f32)>,
}

fn main() {
    let backend =
        slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    // Pure black and white only, so every update is flash-free.
    backend.set_black_and_white(true);

    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    // Dab positions for the UI; push/remove drive the view directly.
    let dabs = Rc::new(VecModel::<Dab>::default());
    app.set_dabs(dabs.clone().into());
    let trail = Rc::new(RefCell::new(Trail::default()));

    // The timer only runs while there's a trail to age; it stops itself once
    // empty, so an idle device blocks in poll(2) instead of waking to do nothing.
    let timer = Rc::new(Timer::default());

    app.on_paint({
        let dabs = dabs.clone();
        let trail = trail.clone();
        let timer = timer.clone();
        let app_weak = app.as_weak();
        move |x, y| {
            // Lay dabs along the segment from the previous sample so a fast move
            // doesn't leave gaps between the touch controller's (relatively
            // sparse) reported positions. Spacing is ~half a dab width; the
            // interpolated dabs share one timestamp, so a fast segment also
            // erases fast.
            let dab_width = app_weak
                .upgrade()
                .map_or(0.0, |app| app.window().size().width as f32)
                * 0.045;
            let step = (dab_width * 0.5).max(1.0);

            let now = Instant::now();
            {
                let mut state = trail.borrow_mut();
                // Fresh ink cancels any in-progress erase; the stroke re-anchors
                // once the finger goes still again.
                state.erase_anchor = None;

                match state.last_point {
                    Some((last_x, last_y)) => {
                        let (dx, dy) = (x - last_x, y - last_y);
                        let distance = (dx * dx + dy * dy).sqrt();
                        let count = (distance / step).ceil().max(1.0) as usize;
                        for i in 1..=count {
                            let along = i as f32 / count as f32;
                            dabs.push(Dab { x: last_x + dx * along, y: last_y + dy * along });
                            state.born.push(now);
                        }
                    }
                    None => {
                        dabs.push(Dab { x, y });
                        state.born.push(now);
                    }
                }
                state.last_point = Some((x, y));
            }

            if !timer.running() {
                let dabs = dabs.clone();
                let trail = trail.clone();
                let timer_handle = timer.clone();
                timer.start(TimerMode::Repeated, TICK, move || {
                    let now = Instant::now();
                    let mut trail = trail.borrow_mut();

                    let Some(&last) = trail.born.last() else {
                        timer_handle.stop();
                        return;
                    };

                    // Hold the whole stroke until the finger has been still for
                    // ERASE_DELAY. While drawing, `last` keeps advancing, so we
                    // never get past here and nothing is erased yet.
                    if now < last + ERASE_DELAY {
                        return;
                    }

                    let anchor = match trail.erase_anchor {
                        Some(anchor) => anchor,
                        None => {
                            let anchor = trail.born[0];
                            trail.erase_anchor = Some(anchor);
                            anchor
                        }
                    };

                    // Remove every dab whose turn has come. A dab drawn `offset`
                    // after the stroke's first dab is removed `offset` after the
                    // erase begins, so the trail rewinds at drawing speed.
                    let erase_start = last + ERASE_DELAY;
                    while let Some(&front) = trail.born.first() {
                        if now < erase_start + (front - anchor) {
                            break;
                        }
                        trail.born.remove(0);
                        dabs.remove(0);
                    }

                    if trail.born.is_empty() {
                        trail.erase_anchor = None;
                        timer_handle.stop();
                    }
                });
            }
        }
    });

    app.on_released({
        let trail = trail.clone();
        move || {
            // Stroke ended — forget the last point so the next stroke doesn't
            // draw a line connecting across from it.
            trail.borrow_mut().last_point = None;
        }
    });

    app.run().expect("event loop error");
}
