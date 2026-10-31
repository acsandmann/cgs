//! SkyLight windows, layer surfaces and transactions, plus the Metal-backed `preview`
//! compositor and the `switcher` built on it.

mod backdrop;
mod layer;
mod private;
pub use private::G_CONNECTION;
mod surface;
mod transaction;
mod window;
pub use backdrop::backdrop_blur;
use backdrop::blur_filters;
pub(crate) use backdrop::{progressive_backdrop, backdrop_scale};
pub use layer::{
    LayerTransaction, release_layer_tree, render_layer_to_cgs_window, with_disabled_actions,
};
pub use surface::WindowSurface;
pub use transaction::WindowTransaction;
pub use window::{CgsWindow, CgsWindowError};
fn cg_ok(error: objc2_core_graphics::CGError) -> Result<(), objc2_core_graphics::CGError> {
    if error == objc2_core_graphics::CGError::Success {
        Ok(())
    } else {
        Err(error)
    }
}

pub fn main_connection() -> i32 { unsafe { private::SLSMainConnectionID() } }

pub mod preview;
pub mod switcher;
