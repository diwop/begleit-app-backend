/// A macro to dynamically bridge multiple tonic async_trait service methods into Axum JSON routes.
///
/// This simplifies keeping REST JSON and gRPC endpoints in sync by taking existing tonic
/// state and methods, wrapping them in an Axum adapter handling the JSON -> gRPC Payload conversion,
/// and automatically mounting them.
///
/// # Arguments
/// * `$router` - The base `axum::Router` instance to append the new routes to.
/// * `$prefix` - A string literal defining the prefix path for the wrapped routes (e.g., `"/management"`).
/// * `$state` - The shared state variable representing the gRPC backend implementation instance.
/// * `$state_type` - The concrete Rust type of the given `$state` necessary for the `axum::extract::State` signature.
/// * `[$( $method:ident ),*]` - An array-like list of existing async methods on the `$state` that should
///   be seamlessly exposed as JSON endpoints.
///
/// # Route Naming
/// The macro automatically maps methods written in `snake_case` to HTTP endpoints in `kebab-case`.
/// For instance, providing `list_users` maps to `POST <prefix>/list-users`.
#[macro_export]
macro_rules! route_grpc_json_service {
    ($router:expr, $prefix:expr, $state:expr, $state_type:ty, [ $( $method:ident ),* $(,)? ]) => {
        {
            let mut router = $router;
            $(
                // Convert to a string and use it directly for the route, replacing snake_case with kebab-case, e.g. "/management/list-users"
                let path = format!("{}/{}", $prefix, stringify!($method).replace("_", "-"));

                router = router.route(
                    &path,
                    axum::routing::post(
                        |axum::extract::State(state): axum::extract::State<$state_type>,
                         axum::Json(payload)| async move {
                            let req = tonic::Request::new(payload);
                            let res = state
                                .$method(req)
                                .await
                                .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

                            use axum::response::IntoResponse;
                            Ok::<axum::response::Response, axum::http::StatusCode>(axum::Json(res.into_inner()).into_response())
                        },
                    )
                    .with_state($state.clone()),
                );
            )*
            router
        }
    };
}
