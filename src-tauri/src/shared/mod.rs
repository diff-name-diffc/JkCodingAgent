pub(crate) mod error;
mod state;
mod text;

pub(crate) use state::TaskManager;
pub use text::truncate_for_display;
