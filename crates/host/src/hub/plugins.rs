//! `/plugin` 的端口命令（docs/design/plugins.md §3）。

use super::*;

impl Hub {
    pub(super) async fn plugin_command(&self, _cmd: Command) -> Result<Reply, PortError> {
        Err(PortError::Failed("plugins are not available yet".into()))
    }
}
