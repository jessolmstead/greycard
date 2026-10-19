//! The adapter the window draws on, and so where the engine's ops
//! run.
//!
//! Slint takes no CPU adapter unless `SLINT_WGPU_CPU` is set, and
//! without a hardware one it fails the start with "no GPU-backed WGPU
//! adapter". It draws through Vulkan (Metal on a Mac, DX12 or Vulkan on
//! Windows) and never GL, so a GPU only GL can drive counts as none:
//! an Ivy Bridge, whose Mesa Vulkan driver wgpu will not take, is one.
//! Such a machine has a software adapter all the same, Mesa's lavapipe
//! or WARP on Windows. When Slint's select fails we look for one, and
//! where it is all there is, let Slint take it and select again. The
//! window draws there, and the engine's ops run on their CPU paths as
//! `--cpu-ops` puts them: the same ops as shaders on a software adapter
//! took minutes a frame where the CPU paths take seconds. A machine
//! Slint selects on at the first try pays nothing for any of this.

use std::sync::atomic::{AtomicBool, Ordering};

use greycard_gpu::wgpu;

use crate::{App, State};

/// The variable Slint reads to take a CPU adapter.
const SLINT_WGPU_CPU: &str = "SLINT_WGPU_CPU";

/// The variable wgpu reads to take the adapter it names, which Slint
/// honors before its own choice.
const WGPU_ADAPTER_NAME: &str = "WGPU_ADAPTER_NAME";

/// Set beside `SLINT_WGPU_CPU` where we set it: a greycard started
/// from anything we start (a browser `webbrowser` opened, which takes
/// no environment of ours) inherits both, and reads the first as ours
/// and not as asked for by hand.
const OURS: &str = "GREYCARD_SET_SLINT_WGPU_CPU";

/// Whether we set `SLINT_WGPU_CPU` ourselves, and so take it off the
/// commands we start that let us.
static WE_SET: AtomicBool = AtomicBool::new(false);

/// The graphics interface the window draws through here.
const API: &str = if cfg!(target_vendor = "apple") {
    "Metal"
} else if cfg!(target_family = "windows") {
    "Vulkan or DX12"
} else {
    "Vulkan"
};

/// Where a software adapter comes from here.
const SOFTWARE_FROM: &str = if cfg!(target_vendor = "apple") {
    ""
} else if cfg!(target_family = "windows") {
    " (WARP gives one)"
} else {
    " (Mesa's lavapipe gives one)"
};

/// The start's error where there is no adapter at all.
pub(crate) fn nothing_error() -> String {
    format!("no {API} adapter wgpu can use, not even a software one{SOFTWARE_FROM}")
}

/// Where the window draws, and so where the engine's ops run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawing {
    /// Slint selected on its own, as it always has, or the probe after
    /// its failure found a GPU it would take: the engine's ops run on
    /// the window's device, unless that is a software adapter.
    Gpu,
    /// No GPU greycard can draw on, a software adapter: the window
    /// draws there and the engine's ops run on the CPU.
    Software,
    /// `SLINT_WGPU_CPU` or `WGPU_ADAPTER_NAME` set by whoever started
    /// us: Slint's and wgpu's own meaning, and the engine's ops on
    /// whichever adapter the window is given, a software one included.
    /// The way to run the window on lavapipe with the ops as shaders
    /// there, which a check of the shaders in the window wants.
    AsAsked,
    /// No adapter at all, not even a software one.
    Nothing,
}

/// What the status line says at the start where the window draws in
/// software, and what every develop's line carries after its time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lines {
    pub(crate) start: String,
    pub(crate) tail: &'static str,
}

impl Drawing {
    /// Whether the window draws in software, with the engine's ops on
    /// the CPU, given the kind of adapter its device was made on: the
    /// device decides, so a GPU that Slint passed over for a software
    /// adapter (one that cannot present to the window, say) is software
    /// too. Never where the adapter was asked for by hand.
    pub(crate) fn in_software(self, window: wgpu::DeviceType) -> bool {
        self != Drawing::AsAsked && window == wgpu::DeviceType::Cpu
    }

    /// The lines for a window on `window`'s kind of adapter, named
    /// `name`; none where it does not draw in software. "No GPU" only
    /// where the probe found none: a machine with one whose window
    /// still landed on a software adapter is told that instead.
    pub(crate) fn lines(self, window: wgpu::DeviceType, name: &str) -> Option<Lines> {
        if !self.in_software(window) {
            return None;
        }
        Some(if self == Drawing::Software {
            Lines {
                start: format!(
                    "no GPU greycard can draw on through {API}: drawing in software and \
                     developing on the CPU, which is slow"
                ),
                tail: " on the CPU, no usable GPU",
            }
        } else {
            Lines {
                start: format!(
                    "drawing in software on {name} and developing on the CPU, which is slow"
                ),
                tail: " on the CPU, drawing in software",
            }
        })
    }
}

/// Whether the adapter was asked for by hand, through either variable.
pub(crate) fn asked() -> bool {
    asked_in(|name| std::env::var_os(name))
}

/// [`asked`] over an environment `var` reads: `SLINT_WGPU_CPU` that a
/// greycard set, with its marker beside it, is not asked for by hand.
fn asked_in(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    let slint_cpu = var(SLINT_WGPU_CPU).is_some_and(|v| v.to_str().is_some());
    (slint_cpu && var(OURS).is_none()) || var(WGPU_ADAPTER_NAME).is_some()
}

/// The choice from what the probe found and whether the adapter was
/// asked for by hand: any hardware adapter is a GPU, as Slint's own
/// check has it.
pub(crate) fn decide(found: &[wgpu::DeviceType], asked: bool) -> Drawing {
    if asked {
        Drawing::AsAsked
    } else if found.iter().any(|t| *t != wgpu::DeviceType::Cpu) {
        Drawing::Gpu
    } else if found.is_empty() {
        Drawing::Nothing
    } else {
        Drawing::Software
    }
}

/// The backends Slint's check looks on, given those asked for (what
/// `WGPU_BACKEND` names, or the default): those less the ones Slint
/// does not draw with here (GL everywhere, Metal off Apple, DX12 off
/// Windows), unless that would leave none. Slint's own
/// `mask_backends` and `default_backends_to_avoid`, which it does not
/// export.
fn backends(requested: wgpu::Backends) -> wgpu::Backends {
    let mut avoid = wgpu::Backends::GL;
    if !cfg!(target_vendor = "apple") {
        avoid |= wgpu::Backends::METAL;
    }
    if !cfg!(target_family = "windows") {
        avoid |= wgpu::Backends::DX12;
    }
    let masked = requested & !avoid;
    if masked.is_empty() { requested } else { masked }
}

/// After Slint's select failed with no adapter asked for by hand: what
/// is there, said in the log, and where only a software adapter is,
/// the variable Slint's check reads set, for a second select. Called
/// on the window's thread, once.
pub(crate) fn probe() -> Drawing {
    let started = std::time::Instant::now();
    let found = greycard_gpu::adapters(backends(wgpu::Backends::from_env().unwrap_or_default()));
    let types: Vec<_> = found.iter().map(|i| i.device_type).collect();
    let drawing = decide(&types, false);
    let names = found
        .iter()
        .map(|i| format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type))
        .collect::<Vec<_>>()
        .join(", ");
    tracing::info!(
        "Slint's select failed; on {API}: {} in {:.0} ms",
        if names.is_empty() { "none" } else { &names },
        started.elapsed().as_secs_f64() * 1000.0
    );
    if drawing == Drawing::Software {
        // The warning comes once the window's device is made, with the
        // adapter Slint took.
        tracing::info!(
            "no GPU greycard can draw on through {API}: letting Slint take a software one"
        );
        // SAFETY: on the window's thread, before the event loop, the
        // worker, or any pool of ours starts, so no thread of ours reads
        // the environment meanwhile. The drivers' own threads end with
        // the instances that made them, Slint's failed probe's and ours,
        // both dropped by now: a breakpoint on setenv found the main
        // thread alone under lavapipe, and alone again with NVIDIA's,
        // RADV, and lavapipe all loaded. Slint's check reads it next,
        // on this thread. Slint's `Manual` configuration would need it
        // all the same: its check refuses an instance with only a CPU
        // adapter however the adapter is handed over.
        unsafe {
            std::env::set_var(SLINT_WGPU_CPU, "1");
            std::env::set_var(OURS, "1");
        }
        WE_SET.store(true, Ordering::Relaxed);
    }
    drawing
}

/// Whether the engine's ops are handed the window's device: not in
/// software, where their shaders would crawl, nor under `--cpu-ops`.
pub(crate) fn lend_device(in_software: bool, cpu_ops: bool) -> bool {
    !in_software && !cpu_ops
}

/// A command we start, without the variable where we set it: a greycard
/// it starts in turn (a file manager we opened, and a raw opened from
/// it) would otherwise take it as asked for by hand and run its ops as
/// shaders on the software adapter.
pub(crate) fn scrub(command: &mut std::process::Command) -> &mut std::process::Command {
    if WE_SET.load(Ordering::Relaxed) {
        command.env_remove(SLINT_WGPU_CPU).env_remove(OURS);
    }
    command
}

/// Once the window's device is made: the lines where it draws in
/// software, kept for the develop's line and said in the status line,
/// and in the log with the adapter's own line, `gpu`.
pub(crate) fn show(app: &App, st: &mut State, lines: Option<Lines>, gpu: &str) {
    st.develop.software_tail = lines.as_ref().map(|l| l.tail);
    if let Some(lines) = lines {
        tracing::warn!(
            "{} (engine ops on the CPU, the window on {gpu})",
            lines.start
        );
        app.set_status(lines.start.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};

    #[test]
    fn a_hardware_adapter_is_a_gpu_whatever_is_beside_it() {
        for found in [
            &[DiscreteGpu][..],
            &[IntegratedGpu, Cpu],
            &[Cpu, VirtualGpu],
            &[Other],
        ] {
            assert_eq!(decide(found, false), Drawing::Gpu, "{found:?}");
        }
        assert!(!Drawing::Gpu.in_software(DiscreteGpu));
        assert!(!Drawing::Gpu.in_software(IntegratedGpu));
        assert_eq!(Drawing::Gpu.lines(DiscreteGpu, "RTX"), None);
    }

    #[test]
    fn a_software_adapter_alone_draws_in_software_with_the_ops_on_the_cpu() {
        // lavapipe, or lavapipe and llvmpipe's GL had GL been probed.
        for found in [&[Cpu][..], &[Cpu, Cpu]] {
            let drawing = decide(found, false);
            assert_eq!(drawing, Drawing::Software, "{found:?}");
            assert!(drawing.in_software(Cpu));
        }
        let lines = Drawing::Software.lines(Cpu, "llvmpipe").unwrap();
        assert!(
            lines
                .start
                .starts_with("no GPU greycard can draw on through "),
            "{lines:?}"
        );
        assert_eq!(lines.tail, " on the CPU, no usable GPU");
    }

    /// A GPU there, and the window given a software adapter all the
    /// same: the ops go to the CPU, and the lines name the adapter
    /// rather than say there is no GPU.
    #[test]
    fn the_window_on_a_software_adapter_beside_a_gpu_is_not_told_no_gpu() {
        assert!(Drawing::Gpu.in_software(Cpu));
        let lines = Drawing::Gpu.lines(Cpu, "llvmpipe").unwrap();
        assert_eq!(
            lines.start,
            "drawing in software on llvmpipe and developing on the CPU, which is slow"
        );
        assert!(!lines.start.contains("no GPU"), "{lines:?}");
        assert!(!lines.tail.contains("GPU"), "{lines:?}");
    }

    #[test]
    fn nothing_found_is_nothing() {
        assert_eq!(decide(&[], false), Drawing::Nothing);
        let said = nothing_error();
        assert!(said.contains("not even a software one"), "{said}");
        assert!(!said.contains("SLINT_WGPU_CPU"), "{said}");
    }

    /// An adapter asked for by hand keeps Slint's and wgpu's meaning,
    /// and the ops stay on the window's device even where that is
    /// lavapipe: what a check of the shaders in the window runs under.
    #[test]
    fn an_adapter_asked_for_by_hand_keeps_the_ops_on_the_device() {
        for found in [&[][..], &[Cpu], &[DiscreteGpu, Cpu]] {
            let drawing = decide(found, true);
            assert_eq!(drawing, Drawing::AsAsked, "{found:?}");
            assert!(!drawing.in_software(Cpu));
            assert_eq!(drawing.lines(Cpu, "llvmpipe"), None);
        }
    }

    /// The device is lent to the worker only off a software adapter and
    /// without `--cpu-ops`: derived from the drawing as the start does,
    /// a window in software never hands it over.
    #[test]
    fn the_device_is_lent_only_on_a_gpu_without_cpu_ops() {
        for (drawing, window, cpu_ops, lent) in [
            (Drawing::Gpu, DiscreteGpu, false, true),
            (Drawing::Gpu, DiscreteGpu, true, false),
            (Drawing::Software, Cpu, false, false),
            (Drawing::Software, Cpu, true, false),
            (Drawing::Gpu, Cpu, false, false),
            (Drawing::AsAsked, Cpu, false, true),
            (Drawing::AsAsked, Cpu, true, false),
        ] {
            assert_eq!(
                lend_device(drawing.in_software(window), cpu_ops),
                lent,
                "{drawing:?} on {window:?}, --cpu-ops {cpu_ops}"
            );
        }
    }

    /// GL is left out where something else is asked, as Slint leaves
    /// it out, and kept where it is all that is asked.
    #[test]
    fn the_probe_looks_where_slint_looks() {
        let all = backends(wgpu::Backends::all());
        assert!(!all.contains(wgpu::Backends::GL), "{all:?}");
        assert!(all.contains(wgpu::Backends::VULKAN), "{all:?}");
        assert_eq!(
            backends(wgpu::Backends::VULKAN | wgpu::Backends::GL),
            wgpu::Backends::VULKAN
        );
        assert_eq!(backends(wgpu::Backends::GL), wgpu::Backends::GL);
    }

    /// The variable and its marker stay on a command until we set them
    /// ourselves.
    #[test]
    fn the_variable_we_set_is_taken_off_the_commands_we_start() {
        let removed = |c: &std::process::Command, name: &str| {
            c.get_envs().any(|(k, v)| k == name && v.is_none())
        };
        let mut before = std::process::Command::new("true");
        if !WE_SET.load(Ordering::Relaxed) {
            assert!(!removed(scrub(&mut before), SLINT_WGPU_CPU));
        }
        WE_SET.store(true, Ordering::Relaxed);
        let mut after = std::process::Command::new("true");
        scrub(&mut after);
        assert!(removed(&after, SLINT_WGPU_CPU));
        assert!(removed(&after, OURS));
    }

    /// `SLINT_WGPU_CPU` is asked for by hand unless a greycard set it,
    /// which its marker says; `WGPU_ADAPTER_NAME` always is.
    #[test]
    fn the_variable_a_greycard_set_is_not_asked_for_by_hand() {
        let env = |set: &'static [&'static str]| {
            move |name: &str| set.contains(&name).then(|| std::ffi::OsString::from("1"))
        };
        assert!(!asked_in(env(&[])));
        assert!(asked_in(env(&[SLINT_WGPU_CPU])));
        assert!(!asked_in(env(&[SLINT_WGPU_CPU, OURS])));
        assert!(!asked_in(env(&[OURS])));
        assert!(asked_in(env(&[WGPU_ADAPTER_NAME])));
        assert!(asked_in(env(&[SLINT_WGPU_CPU, OURS, WGPU_ADAPTER_NAME])));
    }

    /// A develop landed, its line as the status line shows it.
    fn develop_line(app: &App, state: &std::cell::RefCell<State>) -> String {
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                kind: crate::finish::Source::SCENE,
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 60,
                        height: 40,
                        pixels: Vec::new(),
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 13.7,
                detail: None,
                sharpen: None,
                dehaze: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
        app.get_status().to_string()
    }

    /// In software the status line says so at the start, and every
    /// develop's line after says where the time went.
    #[test]
    fn the_status_line_says_where_the_window_draws_in_software() {
        use crate::testing::{folder, state_for, window};
        let app = window(1);
        let (state, _worker) = state_for(&app, folder(1));
        app.set_status("".into());
        let lines = Drawing::Software.lines(Cpu, "llvmpipe");
        show(
            &app,
            &mut state.borrow_mut(),
            lines.clone(),
            "llvmpipe (Vulkan, Cpu)",
        );
        assert_eq!(app.get_status(), lines.unwrap().start);
        assert_eq!(
            develop_line(&app, &state),
            "60x40, developed in 13.70 s on the CPU, no usable GPU"
        );
    }

    /// On a GPU nothing is said, and the develop's line is as it was.
    #[test]
    fn on_a_gpu_the_status_line_is_left_alone() {
        use crate::testing::{folder, state_for, window};
        let app = window(1);
        let (state, _worker) = state_for(&app, folder(1));
        app.set_status("as it was".into());
        show(
            &app,
            &mut state.borrow_mut(),
            Drawing::Gpu.lines(DiscreteGpu, "RTX"),
            "a GPU (Vulkan, DiscreteGpu)",
        );
        assert_eq!(app.get_status(), "as it was");
        assert_eq!(develop_line(&app, &state), "60x40, developed in 13.70 s");
    }
}
