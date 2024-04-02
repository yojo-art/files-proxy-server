use crate::{ConfigFile, PingStatus};


pub(crate) fn cli(subcommand:String,_args: Vec<String>,config:ConfigFile){
	match subcommand.as_str(){
		"ping"=>{
			tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
				let error=PingStatus{
					ms:0,
					ok:false,
				};
				let resp=reqwest::get(format!("http://{}/ping",config.http_bind_addr)).await;
				let resp=match resp{
					Ok(v)=>v,
					Err(e)=>{
						eprintln!("{:?}",e);
						println!("{}",serde_json::to_string(&error).unwrap());
						std::process::exit(1);
					}
				};
				let bytes=resp.bytes().await;
				let bytes=match bytes{
					Ok(v)=>v,
					Err(e)=>{
						eprintln!("{:?}",e);
						println!("{}",serde_json::to_string(&error).unwrap());
						std::process::exit(2);
					}
				};
				let json=serde_json::from_slice::<PingStatus>(&bytes);
				let json=match json{
					Ok(v)=>v,
					Err(e)=>{
						eprintln!("{:?}",e);
						println!("{}",serde_json::to_string(&error).unwrap());
						std::process::exit(3);
					}
				};
				println!("{}",serde_json::to_string(&json).unwrap());
				if json.ok{
					std::process::exit(0);
				}else{
					std::process::exit(4);
				}
			});
		},
		_=>{}
	}
}
