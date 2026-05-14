pub(crate) mod cache;
pub(crate) mod chunk;
pub(crate) mod collection;
pub(crate) mod document;
pub(crate) mod embed;
pub(crate) mod graph;
pub(crate) mod ingest;
pub(crate) mod jobs;
pub(crate) mod pages;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod types;
pub(crate) mod utils;
pub mod vector_store;

pub(crate) const COLLECTIONS_FILE: &str = "collections.json";

// Public type re-exports

// Public Tauri command re-exports
pub use pages::read_page_for_agent;
pub use search::search_for_agent;
pub use utils::set_resource_dir_hint;
