use anyhow::Result;

pub(crate) type CommandResult<T> = std::result::Result<T, String>;

pub(crate) trait IntoCommandResult<T> {
    fn into_command_result(self) -> CommandResult<T>;
}

impl<T> IntoCommandResult<T> for Result<T> {
    fn into_command_result(self) -> CommandResult<T> {
        self.map_err(|error| error.to_string())
    }
}
