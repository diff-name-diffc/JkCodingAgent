use chrono::Local;

pub(super) fn current_local_time() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}
