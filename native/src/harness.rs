//! Scripted-input test harness. Compiled only with `--features harness`, so the
//! shipped binary carries none of it.
//!
//! `MW_SCRIPT` names a file of one command per line, each applied to a single
//! redraw. `shot <path>` writes the composed frame to a PNG **from our own
//! pixmap** — it never reads the screen, so it can only ever contain this
//! window — alongside a `.txt` of the app's state for assertions.
//!
//! ```text
//! move <x> <y> | down | up | click | dclick | wheel <d>
//! ctrl 0|1 | shift 0|1 | key <name> | text <s>
//! wait <frames> | shot <path> | exit
//! ```

use crate::ui::Key;
use crate::Host;
use winit::event_loop::ActiveEventLoop;

pub struct Harness {
    commands: Vec<String>,
    pc: usize,
    wait: u32,
}

impl Harness {
    pub fn new() -> Harness {
        let commands = std::env::var("MW_SCRIPT")
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| {
                t.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect()
            })
            .unwrap_or_default();
        Harness { commands, pc: 0, wait: 0 }
    }

    pub fn running(&self) -> bool {
        self.pc < self.commands.len()
    }
}

pub fn step(host: &mut Host, event_loop: &ActiveEventLoop) {
    if !host.harness.running() {
        return;
    }
    if host.harness.wait > 0 {
        host.harness.wait -= 1;
        return;
    }
    let line = host.harness.commands[host.harness.pc].clone();
    host.harness.pc += 1;

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let num = |p: Option<&str>| p.and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);

    // `shot` needs the app as well as the context, so it is handled before the
    // context is borrowed for everything else.
    if cmd == "shot" {
        if let (Some(path), Some(ctx)) = (parts.next(), host.ctx.as_ref()) {
            let _ = ctx.painter.pixmap.save_png(path);
            let (w, h) = ctx.painter.size();
            let scale = ctx.painter.scale();
            let state = host.app.as_ref().map(|a| a.debug_state()).unwrap_or_default();
            let prof: Vec<String> = ctx.prof.iter().map(|(n, ms)| format!("{}={:.1}", n, ms)).collect();
            let _ = std::fs::write(
                format!("{}.txt", path),
                format!(
                    "logical={:.0}x{:.0} scale={} frame={:.1}ms focus={} [{}] {}\n",
                    w,
                    h,
                    scale,
                    ctx.frame_ms,
                    ctx.focus,
                    prof.join(" "),
                    state
                ),
            );
        }
        return;
    }
    // Opening by id keeps scripts independent of where a tile happens to sit.
    if cmd == "open" {
        if let (Some(id), Some(app)) = (parts.next(), host.app.as_mut()) {
            app.open_project(id);
        }
        return;
    }
    if cmd == "exit" {
        event_loop.exit();
        return;
    }
    if cmd == "wait" {
        host.harness.wait = num(parts.next()) as u32;
        return;
    }
    if cmd == "click" || cmd == "dclick" {
        host.harness.commands.insert(host.harness.pc, "up".into());
    }

    let Some(ctx) = host.ctx.as_mut() else { return };
    match cmd {
        "move" => {
            let (x, y) = (num(parts.next()), num(parts.next()));
            ctx.mouse_delta = (x - ctx.mouse.0, y - ctx.mouse.1);
            ctx.mouse = (x, y);
        }
        "down" | "click" | "dclick" => {
            ctx.mouse_down = true;
            ctx.mouse_pressed = true;
            ctx.double_click = cmd == "dclick";
        }
        "up" => {
            ctx.mouse_down = false;
            ctx.mouse_released = true;
        }
        "wheel" => ctx.wheel = num(parts.next()),
        "ctrl" => ctx.mods.ctrl = parts.next() == Some("1"),
        "shift" => ctx.mods.shift = parts.next() == Some("1"),
        "key" => {
            let key = match parts.next().unwrap_or("") {
                "enter" => Some(Key::Enter),
                "escape" => Some(Key::Escape),
                "space" => Some(Key::Space),
                "backspace" => Some(Key::Backspace),
                "delete" => Some(Key::Delete),
                "left" => Some(Key::Left),
                "right" => Some(Key::Right),
                other => other.chars().next().map(Key::Char),
            };
            if let Some(k) = key {
                ctx.keys.push(k);
            }
        }
        "text" => {
            let rest: Vec<&str> = parts.collect();
            ctx.text.push_str(&rest.join(" "));
        }
        _ => {}
    }
}
