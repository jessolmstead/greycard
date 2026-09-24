#!/usr/bin/env python3
"""Generate src/difftest.rs: a differential fuzz of App's .slint tree.

The same file goes into an export of master and of the branch. It
drives the window with a seeded sequence of pointer, wheel and key
events on the testing backend and writes, after every event, every
readable public property that changed and every impure callback that
fired. The two traces must be identical if the split is a pure move.
"""
import re, sys

pub = open(sys.argv[1]).read().splitlines()
out = sys.argv[2]

SIMPLE = {'bool', 'int', 'float', 'string', 'length', 'color'}
ARR = {'[bool]', '[float]', '[string]', '[Pt]', '[FilterChip]', '[MaskBox]', '[MaskPick]', '[CompareTile]'}

props, cbs = [], []
for l in pub:
    m = re.match(r'\s*(in-out|in|out) property ?<(.*?)> ([\w-]+)', l)
    if m:
        props.append((m.group(1), m.group(2), m.group(3).replace('-', '_')))
        continue
    m = re.match(r'\s*(pure )?callback ([\w-]+)(\(([^)]*)\))?( -> (\w+))?', l)
    if m:
        args = [a.strip() for a in (m.group(4) or '').split(',') if a.strip()]
        cbs.append((bool(m.group(1)), m.group(2).replace('-', '_'), args, m.group(6)))

dump = []
for d, t, n in props:
    if t in SIMPLE:
        dump.append(f'    v.push(("{n}", format!("{{:?}}", app.get_{n}())));')
    elif t in ARR:
        dump.append(f'    {{ let m = app.get_{n}(); v.push(("{n}", (0..m.row_count()).map(|i| format!("{{:?}}", m.row_data(i))).collect::<Vec<_>>().join("|"))); }}')

hooks = []
for pure, n, args, ret in cbs:
    names = [f'a{i}' for i in range(len(args))]
    fmt = ' '.join('{:?}' for _ in args)
    if ret is None:
        if pure:
            continue
        hooks.append(f'    {{ let t = t.clone(); app.on_{n}(move |{", ".join(names)}| t.borrow_mut().push(format!("CB {n} {fmt}"{"".join(", " + x for x in names)}))); }}')
    else:
        if n in ('grid_columns', 'grid_slack', 'grid_max_scroll', 'grid_reveal_to'):
            continue  # set by testing::window to the crate's own
        if n == 'custom_edge':
            hooks.append('    app.on_custom_edge(|s| s.trim().parse::<i32>().unwrap_or(0));')
        elif n == 'meta_key':
            hooks.append('    { let t = t.clone(); app.on_meta_key(move |s| { t.borrow_mut().push(format!("CB meta_key {:?}", s)); s.as_str() == "1" }); }')
        else:
            raise SystemExit(f'unhandled returning callback {n}')

rs = r'''//! Differential fuzz (review scratch, never committed).
use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model};

use crate::App;

fn dump(app: &App) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
''' + '\n'.join(dump) + r'''
    v
}

fn hook(app: &App, t: &Rc<RefCell<Vec<String>>>) {
''' + '\n'.join(hooks) + r'''
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 { self.next() % n }
    fn f(&mut self, lo: f32, hi: f32) -> f32 { lo + (self.below(100000) as f32 / 100000.0) * (hi - lo) }
}

const KEYS: &[&str] = &["g", "c", "v", "s", "j", "z", "p", "x", "u", "1", "3", "0", "6", "[", "]", "/", " ", "a", "e", "f", "o", "y", "+", "-"];

fn key(app: &App, text: slint::SharedString) {
    app.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    app.window().dispatch_event(WindowEvent::KeyReleased { text });
}

fn run(seed: u64, steps: usize, scale: f32, wired: bool, trace: &mut Vec<String>) {
    let app = crate::testing::window(if wired { 30 } else { 12 });
    let _keep = if wired { Some(crate::testing::state_for(&app, crate::testing::folder(30))) } else { None };
    if scale != 1.0 {
        app.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    }
    app.window().set_size(slint::LogicalSize::new(1500.0, 950.0));
    app.window().dispatch_event(WindowEvent::Resized { size: slint::LogicalSize::new(1500.0, 950.0) });
    let t = Rc::new(RefCell::new(Vec::new()));
    if !wired { hook(&app, &t); }
    app.set_selected(0);
    match seed % 12 {
        1 => app.set_panel_tab("Crop".into()),
        2 => app.set_panel_tab("Masks".into()),
        3 => { app.set_panel_tab("Masks".into()); app.set_target(1); app.set_adjustment_names(slint::ModelRc::new(slint::VecModel::from(vec![slint::SharedString::from("Global"), "Sky".into()]))); }
        4 => app.set_panel_tab("Retouch".into()),
        5 => app.set_culling(true),
        6 => app.set_grid_open(true),
        7 => app.set_export_open(true),
        8 => app.set_preset_open(true),
        9 => { app.set_fetch_title("t".into()); app.set_fetch_open(true); }
        10 => { app.set_rejects_open(true); }
        11 => { app.set_crop_mode(true); app.set_panel_tab("Crop".into()); }
        _ => {}
    }
    let mut rng = Rng(seed * 2654435761 + 7);
    let mut last: Vec<(&'static str, String)> = Vec::new();
    let w = app.window();
    for step in 0..steps {
        let kind = rng.below(100);
        let (x, y) = if rng.below(2) == 0 { (rng.f(1180.0, 1500.0), rng.f(0.0, 800.0)) } else { (rng.f(0.0, 1500.0), rng.f(0.0, 950.0)) };
        let pos = slint::LogicalPosition::new(x, y);
        let desc;
        if kind < 30 {
            desc = format!("click {x:.1},{y:.1}");
            crate::testing::click(&app, x, y);
        } else if kind < 36 {
            desc = format!("dblclick {x:.1},{y:.1}");
            crate::testing::click(&app, x, y);
            crate::testing::click(&app, x, y);
        } else if kind < 46 {
            let (x2, y2) = (x + rng.f(-200.0, 200.0), y + rng.f(-200.0, 200.0));
            desc = format!("drag {x:.1},{y:.1} -> {x2:.1},{y2:.1}");
            w.dispatch_event(WindowEvent::PointerMoved { position: pos });
            w.dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Left });
            for k in 1..=5 {
                let f = k as f32 / 5.0;
                w.dispatch_event(WindowEvent::PointerMoved { position: slint::LogicalPosition::new(x + (x2 - x) * f, y + (y2 - y) * f) });
            }
            w.dispatch_event(WindowEvent::PointerReleased { position: slint::LogicalPosition::new(x2, y2), button: PointerEventButton::Left });
        } else if kind < 54 {
            let dy = rng.f(-300.0, 300.0);
            desc = format!("wheel {x:.1},{y:.1} {dy:.1}");
            w.dispatch_event(WindowEvent::PointerMoved { position: pos });
            w.dispatch_event(WindowEvent::PointerScrolled { position: pos, delta_x: 0.0, delta_y: dy });
        } else if kind < 60 {
            desc = format!("move {x:.1},{y:.1}");
            w.dispatch_event(WindowEvent::PointerMoved { position: pos });
        } else if kind < 64 {
            desc = format!("rclick {x:.1},{y:.1}");
            w.dispatch_event(WindowEvent::PointerMoved { position: pos });
            w.dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Right });
            w.dispatch_event(WindowEvent::PointerReleased { position: pos, button: PointerEventButton::Right });
        } else if kind < 90 {
            let k: slint::SharedString = match rng.below(12) {
                0 => Key::Escape.into(),
                1 => Key::Return.into(),
                2 => Key::LeftArrow.into(),
                3 => Key::RightArrow.into(),
                4 => Key::UpArrow.into(),
                5 => Key::DownArrow.into(),
                6 => Key::Tab.into(),
                7 => Key::Backspace.into(),
                _ => KEYS[rng.below(KEYS.len() as u64) as usize].into(),
            };
            desc = format!("key {:?}", k);
            key(&app, k);
        } else {
            let m: slint::SharedString = if rng.below(2) == 0 { Key::Control.into() } else { Key::Shift.into() };
            let k: slint::SharedString = ["z", "y", "e", "f", "o", "Z", "E"][rng.below(7) as usize].into();
            desc = format!("mod {:?}+{:?}", m, k);
            w.dispatch_event(WindowEvent::KeyPressed { text: m.clone() });
            key(&app, k);
            w.dispatch_event(WindowEvent::KeyReleased { text: m });
        }
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(40));
        trace.push(format!("#{step} {desc}"));
        for c in t.borrow_mut().drain(..) { trace.push(format!("  {c}")); }
        let now = dump(&app);
        for (i, (n, v)) in now.iter().enumerate() {
            if last.get(i).map(|(_, lv)| lv != v).unwrap_or(true) {
                trace.push(format!("  {n} = {v}"));
            }
        }
        last = now;
    }
}

#[test]
fn difftest() {
    let path = std::env::var("DIFFTEST_OUT").expect("DIFFTEST_OUT");
    let seeds: u64 = std::env::var("DIFFTEST_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let steps: usize = std::env::var("DIFFTEST_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    let mut trace = Vec::new();
    for seed in 1..=seeds {
        for (scale, wired) in [(1.0, false), (1.5, false), (1.0, true)] {
            trace.push(format!("=== seed {seed} scale {scale} wired {wired}"));
            run(seed, steps, scale, wired, &mut trace);
        }
    }
    std::fs::write(path, trace.join("\n")).unwrap();
}
'''
open(out, 'w').write(rs)
print(len(dump), 'props dumped,', len(hooks), 'callbacks hooked')
