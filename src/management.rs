use axum::Router;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

fn list_users() -> Vec<String> {
    vec![]
}

// ----------------- gRPC Service -----------------

pub mod management {
    tonic::include_proto!("management");
}

use management::management_server::{Management, ManagementServer};
use management::{ListUsersRequest, ListUsersResponse};

#[derive(Default, Clone)]
pub struct ManagementImpl {}

#[tonic::async_trait]
impl Management for ManagementImpl {
    async fn list_users(
        &self,
        _request: Request<ListUsersRequest>,
    ) -> Result<Response<ListUsersResponse>, Status> {
        debug!("Handling ListUsers request.");
        let users = list_users();
        info!("Handled ListUsers request.");
        Ok(Response::new(ListUsersResponse { users }))
    }
}

// ----------------- Router Setup -----------------

pub fn setup() -> (Router, ManagementServer<ManagementImpl>) {
    let management_service = ManagementImpl::default();

    let rest_router = crate::route_grpc_json_service!(
        Router::new(),
        "/management",
        management_service.clone(),
        ManagementImpl,
        [list_users]
    );

    let grpc_service = ManagementServer::new(management_service);

    (rest_router, grpc_service)
}
