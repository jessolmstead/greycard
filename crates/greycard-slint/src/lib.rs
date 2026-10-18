//! The editor's Slint UI compiled to Rust: the window, its panels and
//! its sheets, written as the `.slint` files under `ui/`, which
//! greycard-ui drives by setting their properties and answering their
//! callbacks. The Rust they compile to is several times the size of
//! the editor's own code, so it is a crate of its own: a change to the
//! editor's code does not compile it again, and it is compiled only
//! when a `.slint` file changes.

slint::include_modules!();

/// `app.slint`'s text, for the editor's tests that read the key
/// handler out of it.
pub const APP_SLINT: &str = include_str!("../ui/app.slint");

/// `panel/mask.slint`'s text, for the editor's tests that check the
/// mask panel's menus against their tables.
pub const MASK_SLINT: &str = include_str!("../ui/panel/mask.slint");

/// The folder of the panels' `.slint` files, for the editor's tests
/// that read each one.
pub const PANEL_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/panel");
