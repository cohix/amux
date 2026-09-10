//! squad daemon frontend — router, listener and HTTP status mapping.
//!
//! WI 0113 F-02 (decision Q4): squad is not an architectural exception. The
//! daemon's runtime is bootstrapped by Layer 2's `SquadDaemonHandles` over
//! Layer 1's `SquadDaemonEngine`; this module owns nothing but the transport.

pub mod routes;
pub mod state;
pub mod unattended;

use std::sync::Arc;

use crate::command::commands::squad::daemon_runtime::SquadDaemonHandles;
use crate::command::error::CommandError;
use crate::frontend::api::serve::{self, ServeOptions};

use self::state::SquadAppState;

/// Serve a bootstrapped squad daemon until shutdown.
///
/// Build the router over the handles, bind, serve, then stop the scheduler.
/// Every decision below the socket — admission, database, reconciliation,
/// scheduling, auth mode — was made before this was called.
pub async fn serve(handles: SquadDaemonHandles) -> Result<(), CommandError> {
    let requested = handles.bind_addr();
    let state = Arc::new(SquadAppState::new(handles));
    let router = routes::build_router(state.clone());

    let bind_state = state.clone();
    let serve_result = serve::serve_router_with_bound(
        router,
        ServeOptions {
            addr: requested,
            tls: None,
            shutdown_grace: std::time::Duration::from_secs(30),
        },
        move |addr| {
            let endpoint = bind_state.handles.publish_endpoint(addr)?;
            *bind_state
                .bound_addr
                .lock()
                .expect("squad bound-address mutex poisoned") = Some(endpoint.clone());
            tracing::info!(endpoint, "squad daemon listening");
            Ok(())
        },
    )
    .await;

    // Serving has stopped, so stop the scheduler and let its in-flight
    // evaluations drain before this returns.
    state.handles.shutdown().await;
    serve_result
}
