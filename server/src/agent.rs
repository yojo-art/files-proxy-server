use std::{collections::HashMap, net::SocketAddr, sync::{atomic::AtomicU32, Arc}};

use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{tcp::{OwnedReadHalf, OwnedWriteHalf}, TcpStream}, sync::{mpsc::Sender, Mutex, RwLock}};

use crate::{AgentResult, ConfigFile, RequestJob};

pub(crate) struct AgentJobManager{
	config:Arc<ConfigFile>,
	workers:RwLock<Vec<Arc<AgentWorker>>>,
}
impl AgentJobManager{
	pub fn new(config:Arc<ConfigFile>)->Self{
		Self{
			config,
			workers:RwLock::new(Vec::new()),
		}
	}
	pub(crate) async fn agent_ping(&self)->(u64,bool){
		let start_time=tokio::time::Instant::now();
		let res=self.send(RequestJob{
			type_id:3,
			localpath: None,
		}).await;
		let ping_time=tokio::time::Instant::now()-start_time;
		(ping_time.as_millis() as u64,res.is_some())
	}
	pub async fn send(&self,job:RequestJob)->Option<AgentResult>{
		//失敗した時はnoneを返す
		let workers=self.workers.read().await;
		//先頭のworkerから順に試行
		for x in workers.iter(){
			if let Ok(id)=x.send_request(&job).await{
				if let Ok(res)=x.wait_by_id(id).await{
					return Some(res);
				}
			}
		}
		None
	}
	pub(crate) async fn agent_worker(&self,(tcp,_addr):(TcpStream,SocketAddr)){
		let a=Arc::new(AgentWorker::new(self.config.clone(),tcp));
		self.workers.write().await.push(a.clone());
		if let Err(e)=a.read_loop().await{
			println!("exit agent session {}",e);
		}
		a.shutdown().await;
	}
}
struct AgentWorker{
	config: Arc<ConfigFile>,
	reader: Mutex<OwnedReadHalf>,
	writer: Mutex<OwnedWriteHalf>,
	wait_list:Mutex<HashMap<u32,Sender<Option<AgentResult>>>>,
	id: AtomicU32,
}
impl AgentWorker{
	fn new(config:Arc<ConfigFile>,tcp: TcpStream)->Self{
		let (reader,writer)=tcp.into_split();
		let id=AtomicU32::new(0);
		Self{
			config,
			reader:Mutex::new(reader),
			writer:Mutex::new(writer),
			wait_list:Mutex::new(HashMap::new()),
			id,
		}
	}
	async fn send_request(&self,req:&RequestJob)->Result<u32,std::io::Error>{
		let mut con=self.writer.lock().await;
		let id=self.id.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
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
		Ok(id)
	}
	async fn wait_by_id(&self,id:u32)->Result<AgentResult,()>{
		let (s,mut r)=tokio::sync::mpsc::channel(1);
		self.wait_list.lock().await.insert(id,s.clone());
		let timeout=self.config.agent_timeout;
		tokio::runtime::Handle::current().spawn(async move{
			tokio::time::sleep(tokio::time::Duration::from_millis(timeout)).await;
			let _=s.send_timeout(None,tokio::time::Duration::from_millis(100)).await;
		});
		r.recv().await.unwrap_or(None).ok_or(())
	}
	async fn shutdown(&self){
		self.wait_list.lock().await.clear();
	}
	async fn read_loop(&self)->Result<(),std::io::Error>{
		loop{
			let result=self.read_response().await?;
			if let Some(t)=self.wait_list.lock().await.get(&result.id){
				let _=t.send_timeout(Some(result),tokio::time::Duration::from_millis(100)).await;
			}
		}
	}
	async fn read_response(&self)->Result<AgentResult,std::io::Error>{
		let mut con=self.reader.lock().await;
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
}
