mod cad;
mod common;
mod filesystem;
mod search;
mod shell;

use super::registry::AgentTool;

pub(super) fn builtin_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        filesystem::read_file_tool(),
        filesystem::write_file_tool(),
        filesystem::edit_file_tool(),
        filesystem::list_dir_tool(),
        cad::cad_get_dwg_summary_tool(),
        cad::cad_query_dwg_entities_tool(),
        cad::cad_compute_geometry_tool(),
        cad::cad_save_review_result_tool(),
        search::glob_tool(),
        search::grep_tool(),
        shell::exec_tool(),
        shell::message_tool(),
    ]
}
