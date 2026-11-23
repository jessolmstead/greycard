//! A mouse's back and forward side buttons over the whole window.
//!
//! Slint hands a press to the topmost TouchArea under the pointer,
//! whatever the button, and nothing beneath it hears it, so no element
//! of ours sees a side button everywhere: over the picture the viewport
//! has it, over the strip a cell, over a pane a slider. We read the
//! window's own events instead, before Slint routes them, and ask the
//! window (`side-button`) whether the press is its: in Cull a step to
//! the frame before or after, as the arrows step. A press taken goes no
//! further, nor does its release; one not taken goes on to Slint as
//! before, so the grid's side buttons still go to the folders visited.
//!
//! We act on the press, as the grid's side buttons do and as the arrows
//! do. A button does not repeat, and a second press without a release
//! between is not taken again.
//!
//! The same hook keeps the window's `menu-up` with Slint's own popups
//! before each event goes on (`menu_up`), so a menu opened while a text
//! box had the focus holds the step here and every surface's press.
//! It also tells the stall watch (`stall`) what it hears. The hook is
//! installed by `window_events`, which holds the window's frames while
//! it is minimized as well.

use slint::ComponentHandle;
use slint::winit_030::EventResult;
use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};

use crate::App;

/// What the side buttons have heard: each button held, and whether its
/// press was taken; Shift as the keyboard last reported it.
#[derive(Debug, Default)]
pub(crate) struct SideButtons {
    held: [bool; 2],
    taken: [bool; 2],
    shift: bool,
}

impl SideButtons {
    /// One of the window's events: a side button's press or release,
    /// or a change of the modifiers. PreventDefault for a press the
    /// window took and for that press's release.
    pub(crate) fn event(&mut self, app: &App, event: &WindowEvent) -> EventResult {
        menu_up(app);
        heard(event);
        match event {
            WindowEvent::ModifiersChanged(m) => {
                self.shift = m.state().shift_key();
            }
            // A button held as the window lost the keyboard may never
            // report its release here. What its press was is kept: a
            // release that does come still goes with its press.
            WindowEvent::Focused(false) => {
                self.held = [false; 2];
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // Not ours, but the section drags': a left release ends
                // any that no header in hand will end (app.slint,
                // `section-release-seen`). It goes on to Slint as ever.
                if *button == MouseButton::Left && *state == ElementState::Released {
                    app.invoke_section_release_seen();
                }
                let i = match button {
                    MouseButton::Back => 0,
                    MouseButton::Forward => 1,
                    _ => return EventResult::Propagate,
                };
                let taken = match state {
                    ElementState::Pressed if !self.held[i] => {
                        self.held[i] = true;
                        self.taken[i] = app.invoke_side_button(i == 0, self.shift);
                        self.taken[i]
                    }
                    ElementState::Pressed => self.taken[i],
                    ElementState::Released => {
                        self.held[i] = false;
                        std::mem::take(&mut self.taken[i])
                    }
                };
                if taken {
                    return EventResult::PreventDefault;
                }
            }
            _ => {}
        }
        EventResult::Propagate
    }
}

/// The window's `menu-up` set from Slint's own list of its popups: a
/// context menu, a sub-menu, or a combo box's list up, tooltips not
/// counting. The keys' focus says so only when they had it as the
/// menu opened; a menu opened from a text box or a slider took the
/// focus from that instead. Called before each of the window's events
/// is dispatched, so every surface asking `menu-shown` on that event
/// has the answer as it stands; it lags a close only until the next
/// event, and nothing reads it between.
pub(crate) fn menu_up(app: &App) {
    let up = popups(app) > 0;
    if app.get_menu_up() != up {
        app.set_menu_up(up);
    }
}

/// How many of Slint's popups are up on `app`'s window, tooltips not
/// counting.
pub(crate) fn popups(app: &App) -> usize {
    use slint::private_unstable_api::re_exports::{WindowInner, WindowKind};
    WindowInner::from_pub(app.window())
        .active_popups()
        .iter()
        .filter(|p| !matches!(p.window_kind, WindowKind::ToolTip))
        .count()
}

/// What the stall watch (`stall`) hears of the window's events: the
/// presses, the redraws winit delivers, and the focus and occlusion.
fn heard(event: &WindowEvent) {
    use crate::panel::stall;
    let now = std::time::Instant::now;
    match event {
        WindowEvent::MouseInput {
            state: ElementState::Pressed,
            ..
        } => stall::pressed(now()),
        WindowEvent::RedrawRequested => stall::redraw_delivered(now()),
        WindowEvent::Focused(on) => stall::focused(*on, now()),
        WindowEvent::Occluded(on) => stall::occluded(*on, now()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use slint::platform::{Key, PointerEventButton};
    use slint::winit_030::winit::event::{DeviceId, Modifiers};
    use slint::winit_030::winit::keyboard::ModifiersState;

    use super::*;
    use crate::testing::{click_with, folder, press, state_for, window};

    fn button(state: ElementState, back: bool) -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state,
            button: if back {
                MouseButton::Back
            } else {
                MouseButton::Forward
            },
        }
    }

    /// A click of a side button as winit reports it: what the window's
    /// events did with the press and with the release.
    fn side(b: &mut SideButtons, app: &App, back: bool) -> (bool, bool) {
        let prevented = |r| matches!(r, EventResult::PreventDefault);
        let down = prevented(b.event(app, &button(ElementState::Pressed, back)));
        let up = prevented(b.event(app, &button(ElementState::Released, back)));
        (down, up)
    }

    fn steps_of(app: &App) -> Rc<RefCell<Vec<(i32, bool)>>> {
        let steps = Rc::new(RefCell::new(Vec::new()));
        let seen = steps.clone();
        app.on_step(move |by, extend| seen.borrow_mut().push((by, extend)));
        steps
    }

    /// In Cull, back and forward are the arrows' step, taken with their
    /// release; Shift adds, as Shift with an arrow does. A held button
    /// steps once, even should its press come twice.
    #[test]
    fn back_and_forward_step_in_cull() {
        let app = window(5);
        let steps = steps_of(&app);
        let mut b = SideButtons::default();
        app.set_culling(true);
        assert_eq!(side(&mut b, &app, true), (true, true));
        assert_eq!(side(&mut b, &app, false), (true, true));
        // The arrows, for the record of what the buttons match.
        press(&app, Key::LeftArrow);
        press(&app, Key::RightArrow);
        assert_eq!(
            *steps.borrow(),
            [(-1, false), (1, false), (-1, false), (1, false)]
        );
        steps.borrow_mut().clear();
        b.event(
            &app,
            &WindowEvent::ModifiersChanged(Modifiers::from(ModifiersState::SHIFT)),
        );
        side(&mut b, &app, false);
        b.event(
            &app,
            &WindowEvent::ModifiersChanged(Modifiers::from(ModifiersState::empty())),
        );
        side(&mut b, &app, true);
        assert_eq!(*steps.borrow(), [(1, true), (-1, false)]);
        steps.borrow_mut().clear();
        let prevented = |r| matches!(r, EventResult::PreventDefault);
        assert!(prevented(
            b.event(&app, &button(ElementState::Pressed, true))
        ));
        assert!(prevented(
            b.event(&app, &button(ElementState::Pressed, true))
        ));
        assert!(prevented(
            b.event(&app, &button(ElementState::Released, true))
        ));
        assert_eq!(*steps.borrow(), [(-1, false)]);
        // The Cull of a developed frame too.
        app.set_culling(false);
        app.set_cull_developed(true);
        side(&mut b, &app, false);
        assert_eq!(*steps.borrow(), [(-1, false), (1, false)]);
        // Other buttons are Slint's as ever.
        let other = WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Pressed,
            button: MouseButton::Left,
        };
        assert!(matches!(b.event(&app, &other), EventResult::Propagate));
    }

    /// Develop, the Library grid, and the lightbox step nothing, and the
    /// press goes on to Slint: the grid's own side buttons still go to
    /// the folders visited.
    #[test]
    fn nothing_steps_in_develop_or_the_library() {
        use i_slint_backend_testing::ElementHandle;
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(3));
        let steps = steps_of(&app);
        let grid_steps = Rc::new(RefCell::new(0));
        let seen = grid_steps.clone();
        app.on_grid_step(move |_, _, _, _| *seen.borrow_mut() += 1);
        let visits = Rc::new(RefCell::new(Vec::new()));
        let seen = visits.clone();
        app.on_visit_folder(move |back| seen.borrow_mut().push(back));
        let mut b = SideButtons::default();
        assert_eq!(side(&mut b, &app, true), (false, false), "Develop");
        press(&app, "g");
        assert!(app.get_grid_open());
        assert_eq!(side(&mut b, &app, false), (false, false), "the grid");
        press(&app, "l");
        assert!(app.get_lightbox());
        assert_eq!(side(&mut b, &app, true), (false, false), "the lightbox");
        press(&app, "l");
        // Cull with the grid over it is the grid.
        app.set_culling(true);
        assert!(app.get_grid_open());
        assert_eq!(side(&mut b, &app, true), (false, false), "Cull's grid");
        assert!(steps.borrow().is_empty());
        // What Slint does with the press it is passed on: the grid's.
        app.invoke_grid_range(0.0, 773.0, 8, 0.0);
        let cell = ElementHandle::find_by_element_id(&app, "GridSheet::ct")
            .next()
            .expect("a cell on the sheet");
        let (at, size) = (cell.absolute_position(), cell.size());
        click_with(
            &app,
            PointerEventButton::Back,
            at.x + size.width / 2.0,
            at.y + size.height / 2.0,
        );
        assert_eq!(*visits.borrow(), [true]);
        assert!(steps.borrow().is_empty());
        assert_eq!(*grid_steps.borrow(), 0);
    }

    /// A sheet, a menu, or a text field on top: nothing steps under it,
    /// and the press is left to what is on top.
    #[test]
    fn a_sheet_menu_or_field_on_top_keeps_the_frame() {
        let app = window(5);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(5));
        let steps = steps_of(&app);
        let mut b = SideButtons::default();
        app.set_culling(true);
        app.set_keys_open(true);
        assert_eq!(side(&mut b, &app, true), (false, false), "the keys sheet");
        app.set_keys_open(false);
        app.set_export_open(true);
        assert_eq!(side(&mut b, &app, false), (false, false), "export");
        app.set_export_open(false);
        app.global::<crate::Typing>().set_boxes(1);
        assert_eq!(side(&mut b, &app, true), (false, false), "a text box");
        app.global::<crate::Typing>().set_boxes(0);
        app.global::<crate::Typing>().set_editing(true);
        assert_eq!(side(&mut b, &app, true), (false, false), "a typed value");
        app.global::<crate::Typing>().set_editing(false);
        assert!(steps.borrow().is_empty());
        // The frame menu up over the strip.
        let (x, y) = crate::testing::strip_cell(&app, 1);
        click_with(&app, PointerEventButton::Right, x, y);
        assert!(app.get_menu_up());
        assert_eq!(side(&mut b, &app, true), (false, false), "a menu");
        press(&app, Key::Escape);
        assert!(!app.get_menu_up());
        assert!(steps.borrow().is_empty());
        // With all of them gone, the step.
        assert_eq!(side(&mut b, &app, true), (true, true));
        assert_eq!(*steps.borrow(), [(-1, false)]);
    }

    /// A menu opened while a text box has the focus: the box loses it,
    /// not the keys, so `menu-up` is had from Slint's popups as the
    /// next event comes in, and the step waits for the menu to close.
    /// The help menu is opened as its key opens it, which moves no
    /// focus. The grid's side buttons under a tile's menu are
    /// `tree`'s test.
    #[test]
    fn a_menu_opened_from_a_text_box_holds_the_step() {
        use slint::platform::update_timers_and_animations as settle;
        let app = window(5);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(5));
        let steps = steps_of(&app);
        let typing = app.global::<crate::Typing>();
        let popups = || {
            slint::private_unstable_api::re_exports::WindowInner::from_pub(app.window())
                .active_popups()
                .len()
        };
        let mut b = SideButtons::default();
        app.set_culling(true);
        app.invoke_select(1);
        settle();
        let (at, size) = crate::testing::by_id(&app, "MetaSection::title-field");
        crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        settle();
        assert_eq!(typing.get_boxes(), 1, "the box has the focus");
        app.set_help_menu_shown(true);
        settle();
        assert_eq!(popups(), 1, "the help menu");
        assert_eq!(typing.get_boxes(), 0, "the menu took the box's focus");
        assert!(!app.get_menu_up(), "the keys never had it to lose");
        assert_eq!(side(&mut b, &app, true), (false, false));
        assert!(app.get_menu_up(), "had from the popups");
        assert!(steps.borrow().is_empty());
        // Closed, the box has its focus back and holds the step as a
        // box does; let go, the step.
        press(&app, Key::Escape);
        settle();
        assert_eq!(popups(), 0);
        assert_eq!(typing.get_boxes(), 1, "back to the box");
        assert_eq!(side(&mut b, &app, true), (false, false), "the box");
        assert!(!app.get_menu_up(), "the menu is gone");
        typing.invoke_close_all();
        settle();
        assert_eq!(side(&mut b, &app, true), (true, true));
        assert_eq!(*steps.borrow(), [(-1, false)]);
    }

    /// A press taken, the window losing the keyboard before its
    /// release (Alt-Tab), and the release coming after all: it goes
    /// with its press, not on to Slint alone.
    #[test]
    fn a_release_after_focus_lost_goes_with_its_press() {
        let app = window(5);
        let steps = steps_of(&app);
        let mut b = SideButtons::default();
        app.set_culling(true);
        let prevented = |r| matches!(r, EventResult::PreventDefault);
        assert!(prevented(
            b.event(&app, &button(ElementState::Pressed, false))
        ));
        b.event(&app, &WindowEvent::Focused(false));
        assert!(prevented(
            b.event(&app, &button(ElementState::Released, false))
        ));
        // A press after it is a press of its own.
        assert_eq!(side(&mut b, &app, false), (true, true));
        assert_eq!(*steps.borrow(), [(1, false), (1, false)]);
    }

    /// A slider holding the keys is not stepped under, as the picture
    /// does not take the keys from it.
    #[test]
    fn a_slider_holding_the_keys_keeps_the_frame() {
        let app = window(5);
        let steps = steps_of(&app);
        let mut b = SideButtons::default();
        app.set_culling(true);
        let sliders = app.global::<crate::SliderKeys>();
        sliders.set_holder(1);
        assert_eq!(side(&mut b, &app, true), (false, false));
        sliders.set_holder(0);
        assert_eq!(side(&mut b, &app, true), (true, true));
        assert_eq!(*steps.borrow(), [(-1, false)]);
    }

    /// The ends as the arrows have them: back on the first frame and
    /// forward on the last stay where they are, and each step lands
    /// where the arrow's would, through the browser's own step.
    #[test]
    fn the_ends_hold_as_the_arrows_do() {
        let app = window(3);
        let (_state, _worker) = state_for(&app, folder(3));
        let mut b = SideButtons::default();
        app.set_culling(true);
        app.set_selected(0);
        side(&mut b, &app, true);
        assert_eq!(app.get_selected(), 0, "back on the first");
        side(&mut b, &app, false);
        assert_eq!(app.get_selected(), 1);
        side(&mut b, &app, false);
        side(&mut b, &app, false);
        assert_eq!(app.get_selected(), 2, "forward on the last");
        // The arrows from the same place land the same.
        press(&app, Key::RightArrow);
        assert_eq!(app.get_selected(), 2);
        side(&mut b, &app, true);
        press(&app, Key::LeftArrow);
        assert_eq!(app.get_selected(), 0);
        press(&app, Key::LeftArrow);
        assert_eq!(app.get_selected(), 0);
    }
}
