mod state;
mod text;

pub(crate) use state::{TaskManager, TaskTerminationIntent};
pub use text::truncate_for_display;
