mod cad;
mod common;
mod dwg;
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
        dwg::cad_ensure_dwg_index_tool(),
        dwg::cad_get_dwg_overview_tool(),
        dwg::cad_get_dwg_summary_tool(),
        dwg::cad_list_dwg_layers_tool(),
        dwg::cad_query_dwg_entities_tool(),
        dwg::cad_get_dwg_entity_detail_tool(),
        dwg::cad_inspect_dwg_region_tool(),
        dwg::cad_get_dwg_viewer_session_tool(),
        dwg::cad_get_dwg_viewport_tool(),
        dwg::cad_set_dwg_issue_markers_tool(),
        dwg::cad_clear_dwg_issue_markers_tool(),
        dwg::cad_control_dwg_viewer_tool(),
        dwg::cad_pick_dwg_viewer_tool(),
        dwg::cad_capture_dwg_viewer_tool(),
        cad::cad_compute_geometry_tool(),
        cad::cad_save_review_result_tool(),
        search::glob_tool(),
        search::grep_tool(),
        shell::exec_tool(),
        shell::message_tool(),
    ]
}
