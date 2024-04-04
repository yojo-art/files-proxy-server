use std::{collections::HashMap, net::SocketAddr, sync::{atomic::{AtomicBool, AtomicU32}, Arc}, time::Duration};

use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{tcp::{OwnedReadHalf, OwnedWriteHalf}, TcpStream}, sync::{mpsc::{error::TryRecvError, Receiver, Sender}, Mutex}};

use crate::{AgentResult, ConfigFile, RequestJob};


pub(crate) async fn agent_ping(send_queue: &Sender<RequestJob>,config:&ConfigFile)->(u64,bool){
	let start_time=tokio::time::Instant::now();
	let (send_result,mut recv_result)=tokio::sync::mpsc::channel(1);
	if let Err(e)=send_queue.send(RequestJob{
		type_id:3,
		localpath: None,
		result:Some(send_result),
	}).await{
		eprintln!("Agent Ping {}",e);
	}
	let mut res=None;
	for _ in 0..config.ping_worker_timeout as i64/config.ping_worker_poll as i64{
		match recv_result.try_recv(){
			Ok(res0)=>{
				res.replace(res0);
				break;
			},
			Err(TryRecvError::Empty)=>{
				tokio::time::sleep(Duration::from_millis(config.ping_worker_poll.into())).await;
			},
			Err(TryRecvError::Disconnected)=>{
				eprintln!("Agent Ping Disconnected");
				break;
			}
		}
	}
	let ping_time=tokio::time::Instant::now()-start_time;
	(ping_time.as_millis() as u64,res.is_some())
}
pub(crate) async fn agent_worker((tcp,_addr):(TcpStream,SocketAddr),mut recv_queue:Receiver<RequestJob>,config:Arc<ConfigFile>)->Receiver<RequestJob>{
	println!("start agent session");
	let id=AtomicU32::new(0);
	async fn req(con:&mut OwnedWriteHalf,req:&RequestJob,id:u32)->Result<(),std::io::Error>{
		con.write_u8(req.type_id).await?;//種別
		con.write_u8(0u8).await?;//padding
		con.write_u32(id).await?;//id
		if let Some(url_string)=req.localpath.as_ref(){
			let url_string=url_string.as_bytes();
			con.write_u16(url_string.len() as u16+2).await?;
			con.write_u16(url_string.len() as u16).await?;
			con.write_all(url_string).await?;
		}else{
			con.write_u16(0).await?;
		}
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
					tokio::time::sleep(Duration::from_millis(config.agent_queue_send_poll.into())).await;
				},
				Err(TryRecvError::Disconnected)=>{
					break;
				}
			}
		}
		if let Some(mut job)=res{
			let id=id.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
			if let Some(res)=job.result.take(){
				working_buffer0.lock().await.insert(id,res);
			}
			if let Err(e)=req(&mut writer,&job,id).await{
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
