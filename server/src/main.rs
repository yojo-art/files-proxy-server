use std::net::SocketAddr;

use axum::{response::IntoResponse, routing::get, Router};

fn main() {
	let addr = SocketAddr::from(([0, 0, 0, 0],8080));
	tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
		let app = Router::new().route("/*path",get(|axum::extract::Path(path):axum::extract::Path<String>|async{
			println!("{}",path);
			(axum::http::StatusCode::OK,path).into_response()
		}));
		axum::Server::bind(&addr).serve(app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
	});
}
