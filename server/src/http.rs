use std::{borrow::Cow, net::SocketAddr, sync::Arc, time::Duration};

use axum::{body::StreamBody, response::IntoResponse, routing::get, Router};
use redis::Commands;
use tokio::sync::{mpsc::{error::TryRecvError, Sender}, Mutex};

use crate::{agent, AgentResult, ConfigFile, PingStatus, RequestJob, ResponseJson};


pub(crate) async fn http_server(send_queue: Arc<Sender<RequestJob>>,config:Arc<ConfigFile>){
	let redis_client=if let Some(redis_addr)=config.redis_addr.as_ref(){
		let redis_client=redis::Client::open(redis_addr.clone());
		match redis_client{
			Ok(v)=>{
				let redis_client=v.get_connection();
				match redis_client{
					Ok(v)=>Some(v),
					Err(e)=>{
						eprintln!("redis connection error {:?}",e);
						None
					}
				}
			},
			Err(e)=>{
				eprintln!("redis client error {:?}",e);
				None
			}
		}
	}else{
		None
	};
	let redis_client=redis_client.map(|v|Arc::new(Mutex::new(v)));
	let http_addr:SocketAddr = config.http_bind_addr.parse().unwrap();
	let client=reqwest::Client::new();
	let config0=config.clone();
	let send_queue0=send_queue.clone();
	let app = Router::new().route("/",get(||async move{
		axum::http::StatusCode::NO_CONTENT
	})).route("/ping",get(||async move{
		let (ms,ok)=agent::agent_ping(&send_queue0,&config0).await;
		let json=PingStatus{
			ms,ok
		};
		let mut header=axum::headers::HeaderMap::new();
		header.insert("Content-Type","application/json".parse().unwrap());
		(axum::http::StatusCode::OK,header,serde_json::to_string(&json).unwrap())
	})).route("/*path",get(|path,headers|get_file(path, headers, client, send_queue, redis_client, config)));
	axum::Server::bind(&http_addr).serve(app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}
async fn get_file(
	axum::extract::Path(path):axum::extract::Path<String>,
	headers:axum::http::HeaderMap,
	client:reqwest::Client,
	send_queue: Arc<Sender<RequestJob>>,
	redis_client: Option<Arc<Mutex<redis::Connection>>>,
	config:Arc<ConfigFile>)->Result<(axum::http::StatusCode,axum::headers::HeaderMap,StreamBody<impl futures::Stream<Item = Result<axum::body::Bytes, reqwest::Error>>>),axum::response::Response>{

	let mut json=if let Some(redis_client)=redis_client.as_ref(){
		let mut redis_client=redis_client.lock().await;
		let value:Option<String>=redis_client.get(&path).ok();
		value
	}else{
		None
	};
	let redis_hit=json.is_some();
	if json.is_none(){//redisキャッシュに無い
		let res=match agent_call(send_queue,&config,path.clone()).await{
			Ok(v) => v,
			Err(e) => return Err(e),
		};
		let res=match res{
			Some(res)=>res,
			None=>return Err((axum::http::StatusCode::GATEWAY_TIMEOUT,"").into_response())
		};
		let res_json=match res.status{
			200=>res.json.or(Some(String::new())),
			204=>Some(String::new()),
			_=>None
		};
		if let Some(redis_client)=redis_client.as_ref(){
			let mut redis_client=redis_client.lock().await;
			if let Some(res_json)=res_json.as_ref(){
				let res=match config.redis_ttl{
					Some(ttl_seconds)=>{
						redis_client.set_ex(&path,res_json,ttl_seconds)
					},
					None=>{
						redis_client.set(&path,res_json)
					}
				};
				match res{
					Ok(v)=>{
						let _:()=v;
					},
					Err(e)=>{
						eprintln!("redis insert error {:?}",e);
					}
				}
			}
		}
		json=res_json;
		if res.status==404{
			let mut resp=(axum::http::StatusCode::NOT_FOUND).into_response();
			resp.headers_mut().append("X-AgentStatus",res.status.to_string().parse().unwrap());
			return Err(resp);
		}
		if res.status!=200{
			let event_id=uuid::Uuid::new_v4().to_string();
			eprintln!("EventId[{}] job Agent Status {}",event_id,res.status);
			let value=event_id.parse().unwrap();
			let mut resp=(axum::http::StatusCode::BAD_GATEWAY).into_response();
			resp.headers_mut().append("X-EventId",value);
			resp.headers_mut().append("X-AgentStatus",res.status.to_string().parse().unwrap());
			return Err(resp);
		}
	}
	if json.is_none(){
		return Err((axum::http::StatusCode::NO_CONTENT).into_response());
	}
	let json=json.unwrap();
	if json.is_empty(){
		return Err((axum::http::StatusCode::NO_CONTENT).into_response());
	}
	let json=serde_json::from_str::<ResponseJson>(&json);
	let json=match json{
		Ok(data)=>data,
		Err(e)=>{
			let event_id=uuid::Uuid::new_v4().to_string();
			eprintln!("EventId[{}] job Agent Json Parse error {:?}",event_id,e);
			let value=event_id.parse().unwrap();
			let mut resp=(axum::http::StatusCode::BAD_GATEWAY).into_response();
			resp.headers_mut().append("X-EventId",value);
			return Err(resp);
		}
	};
	let url=if json.link{
		Cow::Borrowed(&json.uri)
	}else{
		if let Some(local_media_url)=config.local_media_url.as_ref(){
			Cow::Owned(format!("{}{}",local_media_url,path))
		}else{
			return Err((axum::http::StatusCode::NOT_FOUND).into_response());
		}
	};
	println!("GET {}",url);
	let request=client.get(url.as_ref()).build();
	let mut request=match request{
		Ok(request)=>request,
		Err(e)=>return Err((axum::http::StatusCode::BAD_GATEWAY,format!("{:?}",e)).into_response())
	};
	let req_headers=request.headers_mut();
	if let Some(modified_since)=headers.get("If-Modified-Since").map(|v|v.to_str().ok()).unwrap_or(None){
		req_headers.append("If-Modified-Since",modified_since.parse().unwrap());
	}
	if let Some(modified_since)=headers.get("If-None-Match").map(|v|v.to_str().ok()).unwrap_or(None){
		req_headers.append("If-None-Match",modified_since.parse().unwrap());
	}
	if let Some(user_agent)=config.user_agent.as_ref(){
		req_headers.append("User-Agent",user_agent.parse().unwrap());
	}
	let resp=client.execute(request).await;
	let resp=match resp{
		Ok(resp)=>resp,
		Err(e)=>return Err((axum::http::StatusCode::BAD_GATEWAY,format!("{:?}",e)).into_response())
	};
	let status=resp.status();
	let remote_headers=resp.headers();
	let mut headers=axum::headers::HeaderMap::new();
	headers.append("X-FileProxy-Hit",redis_hit.to_string().parse().unwrap());
	headers.append("X-Remote-Status",status.as_u16().to_string().parse().unwrap());
	if json.link{
		headers.append("X-Remote-Url",url.parse().unwrap());
	}
	headers.append("Vary","Origin".parse().unwrap());
	fn add_remote_header(key:&'static str,headers:&mut axum::headers::HeaderMap,remote_headers:&reqwest::header::HeaderMap){
		for v in remote_headers.get_all(key){
			headers.append(key,String::from_utf8_lossy(v.as_bytes()).parse().unwrap());
		}
	}
	add_remote_header("Content-Disposition",&mut headers,remote_headers);
	add_remote_header("Content-Security-Policy",&mut headers,remote_headers);
	add_remote_header("Content-Type",&mut headers,remote_headers);
	add_remote_header("Cache-Control",&mut headers,remote_headers);
	add_remote_header("Last-Modified",&mut headers,remote_headers);
	add_remote_header("Etag",&mut headers,remote_headers);
	let body=StreamBody::new(resp.bytes_stream());
	if status.is_success(){
		Ok((axum::http::StatusCode::OK,headers,body))
	}else if status==reqwest::StatusCode::NOT_MODIFIED{
		Ok((axum::http::StatusCode::NOT_MODIFIED,headers,body))
	}else{
		Ok((axum::http::StatusCode::BAD_GATEWAY,headers,body))
	}
}
async fn agent_call(send_queue: Arc<Sender<RequestJob>>,config:&ConfigFile,path:String)->Result<Option<AgentResult>,axum::response::Response>{
	let type_id=if path.starts_with("webpublic-"){
		1
	}else if path.starts_with("thumbnail-"){
		2
	}else{
		4
	};
	//println!("{}",path);
	let (send_result,mut recv_result)=tokio::sync::mpsc::channel(1);
	let job=RequestJob{
		type_id,
		localpath:Some(path),
		result:Some(send_result),
	};
	match send_queue.send_timeout(job,Duration::from_millis(3000)).await{
		Ok(_) => {

		},
		Err(e) => {
			let event_id=uuid::Uuid::new_v4().to_string();
			eprintln!("EventId[{}] job send error {:?}",event_id,e);
			let value=event_id.parse().unwrap();
			let mut resp=(axum::http::StatusCode::TOO_MANY_REQUESTS).into_response();
			resp.headers_mut().append("X-EventId",value);
			return Err(resp);
		},
	}
	let mut res=None;
	for _ in 0..config.http_agentwait_timeout as i64/config.http_agentwait_poll as i64{
		match recv_result.try_recv(){
			Ok(res0)=>{
				res.replace(res0);
				break;
			},
			Err(TryRecvError::Empty)=>{
				tokio::time::sleep(Duration::from_millis(config.http_agentwait_poll.into())).await;
			},
			Err(TryRecvError::Disconnected)=>{
				return Err((axum::http::StatusCode::GATEWAY_TIMEOUT,"").into_response());
			}
		}
	}
	recv_result.close();
	Ok(res)
}
