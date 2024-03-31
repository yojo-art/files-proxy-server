use std::{collections::HashMap, net::SocketAddr, sync::{atomic::{AtomicBool, AtomicU32}, Arc}, time::Duration};

use axum::{response::IntoResponse, routing::get, Router};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{tcp::{OwnedReadHalf, OwnedWriteHalf},TcpStream}, sync::{mpsc::{error::TryRecvError, Receiver, Sender}, Mutex}};

fn main() {
	let (send_queue,mut recv_queue)=tokio::sync::mpsc::channel::<RequestJob>(16);
	let http_addr = SocketAddr::from(([0, 0, 0, 0],8080));
	let tcp_addr = SocketAddr::from(([0, 0, 0, 0],23725));
	let rt=tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
	rt.spawn(async move{
		match tokio::net::TcpListener::bind(tcp_addr).await{
			Ok(con) => {
				loop{
					let stream=con.accept().await;
					recv_queue=agent_worker(stream.unwrap(),recv_queue).await;
				}
			},
			Err(e) => {
				eprintln!("{:?}",e);
			},
		}
	});
	rt.block_on(async move{
		let app = Router::new().route("/*path",get(|axum::extract::Path(path):axum::extract::Path<String>|async move{
			//println!("{}",path);
			let (send_result,mut recv_result)=tokio::sync::mpsc::channel(1);
			let job=RequestJob{
				localpath:path,
				result:send_result,
			};
			match send_queue.send_timeout(job,Duration::from_millis(3000)).await{
				Ok(_) => {

				},
				Err(e) => {
					let event_id=uuid::Uuid::new_v4().to_string();
					eprintln!("EventId[{}] job send error {:?}",event_id,e);
					let value=event_id.parse().unwrap();
					let mut resp=(axum::http::StatusCode::TOO_MANY_REQUESTS,event_id).into_response();
					resp.headers_mut().append("X-EventId",value);
					return resp;
				},
			}
			let mut res=None;
			for _ in 0..3000/5{
				tokio::time::sleep(Duration::from_millis(5)).await;
				match recv_result.try_recv(){
					Ok(res0)=>{
						res.replace(res0);
						break;
					},
					Err(TryRecvError::Empty)=>{

					},
					Err(TryRecvError::Disconnected)=>{
						return (axum::http::StatusCode::GATEWAY_TIMEOUT,"").into_response();
					}
				}
			}
			recv_result.close();
			match res{
				Some(res)=>{
					match res.json{
						Some(res)=>{
							(axum::http::StatusCode::OK,res).into_response()
						},
						None=>{
							(axum::http::StatusCode::NO_CONTENT).into_response()
						}
					}
				},
				None=>{
					(axum::http::StatusCode::GATEWAY_TIMEOUT,"").into_response()
				}
			}
		}));
		axum::Server::bind(&http_addr).serve(app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
	});
}
async fn agent_worker((tcp,_addr):(TcpStream,SocketAddr),mut recv_queue:Receiver<RequestJob>)->Receiver<RequestJob>{
	println!("start agent session");
	let id=AtomicU32::new(0);
	async fn req(con:&mut OwnedWriteHalf,url_string:&str,id:u32)->Result<(),std::io::Error>{
		//let id=1234567890;
		let url_string=url_string.as_bytes();
		con.write_u8(1u8).await?;//種別
		con.write_u8(0u8).await?;//padding
		con.write_u32(id).await?;//id
		con.write_u16(url_string.len() as u16+2).await?;
		con.write_u16(url_string.len() as u16).await?;
		con.write_all(url_string).await?;
		con.flush().await?;
		Ok(())
	}
	async fn res(con:&mut OwnedReadHalf)->Result<AgentResult,std::io::Error>{
		let status=con.read_u16().await?;
		let id=con.read_u32().await?;
		//println!("eventid {}",id);
		let len=con.read_u16().await? as usize;
		let s=if len>0{
			let mut buf=Vec::with_capacity(len);
			unsafe{
				buf.set_len(len);
			}
			con.read_exact(&mut buf).await?;
			String::from_utf8(buf).map(|s|Some(s)).unwrap_or_else(|_|None)
		}else{
			None
		};
		Ok(AgentResult{
			id,
			status,
			json:s,
		})
	}
	let working_buffer=Arc::new(Mutex::new(HashMap::new()));
	let (mut reader,mut writer)=tcp.into_split();
	let working_buffer0=working_buffer.clone();
	let is_error=Arc::new(AtomicBool::new(false));
	let is_error0=is_error.clone();
	tokio::runtime::Handle::current().spawn(async move{
		loop{
			match res(&mut reader).await{
				Ok(res)=>{
					let id=res.id;
					let sender:Option<Sender<AgentResult>>=working_buffer.lock().await.remove(&id);
					match sender{
						Some(sender)=>{
							//println!("RESPONSE={:?}",res);
							if let Err(e)=sender.send(res).await{
								eprintln!("AgentResult SendError {}",e);
							}
						},
						None=>{
							eprintln!("Drop Agent Response {}",id);
						}
					}
				},
				Err(e)=>{
					eprintln!("agent session error {:?}",e);
					is_error0.store(true,std::sync::atomic::Ordering::Relaxed);
					break;
				}
			}
		}
	});
	loop{
		let mut res=None;
		loop{
			tokio::time::sleep(Duration::from_millis(5)).await;
			if is_error.load(std::sync::atomic::Ordering::Relaxed){
				break;
			}
			match recv_queue.try_recv(){
				Ok(res0)=>{
					res.replace(res0);
					break;
				},
				Err(TryRecvError::Empty)=>{
					//job wait
				},
				Err(TryRecvError::Disconnected)=>{
					break;
				}
			}
		}
		if let Some(job)=res{
			let id=id.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
			working_buffer0.lock().await.insert(id,job.result);
			if let Err(e)=req(&mut writer,&job.localpath,id).await{
				eprintln!("agent session error {:?}",e);
				match writer.shutdown().await{
					Ok(_)=>{
						println!("shutdown agent session");
					},
					Err(e)=>{
						eprintln!("shutdown {:?}",e);
					}
				}
			}
		}else{
			break;
		}
	}
	println!("exit agent session");
	return recv_queue;
}
struct RequestJob{
	localpath:String,
	result:Sender<AgentResult>,
}
#[derive(Debug)]
struct AgentResult{
	id:u32,
	status:u16,
	json:Option<String>,
}