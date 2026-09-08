//! `ApiServerCommandFrontend` impl for the CLI.

use async_trait::async_trait;

use crate::command::commands::api_server::{ApiServerCommandFrontend, ApiServerRuntime};
use crate::command::error::CommandError;
use crate::frontend::cli::command_frontend::CliFrontend;

#[async_trait]
impl ApiServerCommandFrontend for CliFrontend {
    async fn serve_until_shutdown(
        &mut self,
        runtime: ApiServerRuntime,
    ) -> Result<(), CommandError> {
        crate::frontend::api::serve(runtime).await
    }
}
