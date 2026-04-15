mod builtin;
mod context;
mod delegation;
mod registry;

pub use context::ToolContext;
pub use delegation::{
    is_continue_instruction, is_dispatch_instruction, is_exit_instruction,
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
};
pub use registry::ToolRegistry;

impl ToolRegistry {
    pub fn default_tools() -> Self {
        let mut tools = builtin::builtin_tools();
        tools.extend(delegation::delegation_tools());
        Self::new(tools)
    }
}
