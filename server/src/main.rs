use std::{io::Write, net::SocketAddr, sync::Arc, time::Duration};

use agent::AgentJobManager;
use serde::{Deserialize, Serialize};
mod commands;
mod agent;
mod http;

#[derive(Serialize,Deserialize,Debug)]
pub(crate) struct ConfigFile{
	redis_addr:Option<String>,//redis://127.0.0.1:6379
	redis_ttl:Option<u64>,//3600=1h
	http_bind_addr:String,//0.0.0.0:8080
	agent_bind_addr:String,//0.0.0.0:23725
	ping_wait_ms:u64,//0でping無し 標準30000
	agent_timeout:u64,//1000
	user_agent:Option<String>,
	local_media_url:Option<String>,
}

fn main() {
	let config_path=match std::env::var("FILES_PROXY_CONFIG_PATH"){
		Ok(path)=>{
			if path.is_empty(){
				"config.json".to_owned()
			}else{
				path
			}
		},
		Err(_)=>"config.json".to_owned()
	};
	if !std::path::Path::new(&config_path).exists(){
		let default_config=ConfigFile{
			redis_addr:None,
			redis_ttl:Some(3600),
			http_bind_addr: "0.0.0.0:8080".to_owned(),
			agent_bind_addr: "0.0.0.0:23725".to_owned(),
			ping_wait_ms: 30000,
			agent_timeout:1000,
			user_agent: Some("https://github.com/yojo-art/files-proxy".to_owned()),
			local_media_url:Some("http://localhost:3000".to_owned()),
		};
		let default_config=serde_json::to_string_pretty(&default_config).unwrap();
		std::fs::File::create(&config_path).expect("create default config.json").write_all(default_config.as_bytes()).unwrap();
	}
	let config:ConfigFile=serde_json::from_reader(std::fs::File::open(&config_path).unwrap()).unwrap();

	let args: Vec<String> = std::env::args().collect();
	if let Some(subcommand)=args.get(1).cloned(){
		commands::cli(subcommand,args,config);
		return;
	}
	let config=Arc::new(config);
	let rt=tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
	let job_manager=Arc::new(AgentJobManager::new(config.clone()));
	async fn tcp_listener(config:Arc<ConfigFile>,job_manager: Arc<AgentJobManager>){
		let tcp_addr:SocketAddr = config.agent_bind_addr.parse().unwrap();
		match tokio::net::TcpListener::bind(tcp_addr).await{
			Ok(con) => {
				loop{
					let job_manager=job_manager.clone();
					let stream=con.accept().await;
					if let Ok(stream)=stream{
						tokio::runtime::Handle::current().spawn(async move{
							job_manager.agent_worker(stream).await
						});
					}
				}
			},
			Err(e) => {
				eprintln!("{:?}",e);
			},
		}
	}
	rt.spawn(tcp_listener(config.clone(),job_manager.clone()));
	if config.ping_wait_ms>0{
		let send_queue0=job_manager.clone();
		rt.spawn(ping_worker(send_queue0,config.clone()));
	}
	rt.block_on(http::http_server(job_manager,config));
}
async fn ping_worker(send_queue: Arc<AgentJobManager>,config:Arc<ConfigFile>){
	loop{
		tokio::time::sleep(Duration::from_millis(config.ping_wait_ms)).await;
		let (ping_time,is_ok)=send_queue.agent_ping().await;
		let is_ok=if is_ok{"ok"}else{"error"};
		println!("LastAgentPing {}ms {}",ping_time,is_ok);
	}
}
struct RequestJob{
	type_id:u8,
	localpath:Option<String>,
}
#[derive(Debug)]
struct AgentResult{
	id:u32,
	status:u16,
	json:Option<String>,
}
#[derive(Deserialize,Debug)]
struct ResponseJson{
	uri:String,
	link:bool,
}
#[derive(Serialize,Deserialize,Debug)]
pub(crate) struct PingStatus{
	ms:u64,
	ok:bool,
}
