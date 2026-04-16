/// A macro to dynamically bridge multiple tonic async_trait service methods into Axum JSON routes.
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
                            Ok::<_, axum::http::StatusCode>(axum::Json(res.into_inner()))
                        },
                    )
                    .with_state($state.clone()),
                );
            )*
            router
        }
    };
}
