use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, Pool, Postgres};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter}, net::{tcp::{OwnedReadHalf, OwnedWriteHalf}, TcpStream}, sync::mpsc::{Receiver, Sender}};

#[derive(Deserialize,Debug)]
struct ConfigFile{
	pg_user:Option<String>,
	pg_pass:Option<String>,
	pg_host:Option<String>,
	pg_port:Option<u16>,
	pg_database:Option<String>,
	database_url:Option<String>,
	tcp_host:String,
	tcp_port:u16,
}

#[derive(sqlx::FromRow,Debug)]
#[allow(non_snake_case)]
pub struct DriveFile{
	thumbnailAccessKey: String,
	webpublicAccessKey: String,
	isLink:bool,
	src: Option<String>,
}
pub struct Request{
	request_type:u8,
	_reserved:[u8;1],
	id:u32,
	body:Vec<u8>,
}
pub struct Response{
	status:u16,
	id:u32,
	json:Option<String>,
}
fn main() {
	let config_path=match std::env::var("FILES_AGENT_CONFIG_PATH"){
		Ok(path)=>{
			if path.is_empty(){
				"config.json".to_owned()
			}else{
				path
			}
		},
		Err(_)=>"config.json".to_owned()
	};
	let config:ConfigFile=serde_json::from_reader(std::fs::File::open(config_path).unwrap()).unwrap();
	let database_url = match std::env::var("DATABASE_URL"){
		Ok(url)=>url,
		Err(_)=>{
			match config.database_url{
				Some(url)=>url,
				None=>format!("postgresql://{}:{}@{}:{}/{}",config.pg_user.unwrap(),config.pg_pass.unwrap(),config.pg_host.unwrap(),config.pg_port.unwrap(),config.pg_database.unwrap())
			}
		}
	};
	let (req_sender,mut req_receiver)=tokio::sync::mpsc::channel(2);
	let (res_sender,mut res_receiver)=tokio::sync::mpsc::channel(2);

	let rt=tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
	let res_s1=res_sender.clone();
	rt.spawn(async move{
		loop{
			match TcpStream::connect(format!("{}:{}",config.tcp_host,config.tcp_port)).await{
				Ok(con)=>{
					tcp_worker(con,req_sender.clone(),res_s1.clone(),&mut res_receiver).await;
				},
				Err(err)=>{
					eprintln!("{}",err);
				}
			}
			tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
		}
	});
	rt.block_on(async{
		let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url).await.unwrap();
		loop{
			let job:Request=req_receiver.recv().await.unwrap();
			let pool=pool.clone();
			let res_sender=res_sender.clone();
			tokio::runtime::Handle::current().spawn(async move{
				let res=load_from_db(&pool,job).await;
				if let Err(e)=res_sender.send(res).await{
					eprintln!("Response Queue Send Error {}",e);
				}
			});
		}
	});
}
async fn tcp_worker(tcp:TcpStream,req_sender: Sender<Request>,res_sender: Sender<Response>,res_receiver: &mut Receiver<Response>){
	let (reader,writer)=tcp.into_split();
	let mut reader=BufReader::new(reader);
	let mut writer=BufWriter::new(writer);
	tokio::runtime::Handle::current().spawn(async move{
		async fn read_request(reader: &mut BufReader<OwnedReadHalf>)->std::io::Result<Request>{
			let request_type=reader.read_u8().await?;
			let reserved=reader.read_u8().await?;
			let id=reader.read_u32().await?;
			let body_len=reader.read_u16().await? as usize;
			let mut body=vec![0;body_len];
			if body_len>0{
				reader.read_exact(&mut body).await?;
			}
			Ok(Request{
				request_type,_reserved:[reserved],id,body
			})
		}
		loop{
			if let Ok(req)=read_request(&mut reader).await{
				if let Err(e)=req_sender.send(req).await{
					println!("Request Queue Send Error {}",e);
				}
			}else{
				break;
			}
		}
		println!("EndReciv");
		res_sender.send(Response{
			status: 0,
			id:0,
			json: None
		}).await.unwrap();
	});
	loop{
		if let Some(res)=res_receiver.recv().await{
			if res.status==0{
				break;
			}
			async fn write_response(writer: &mut BufWriter<OwnedWriteHalf>,res: Response)->std::io::Result<()>{
				writer.write_u16(res.status).await?;
				writer.write_u32(res.id).await?;
				if let Some(json)=res.json{
					writer.write_u16(json.len() as u16).await?;
					writer.write_all(json.as_bytes()).await?;
				}else{
					writer.write_u16(0u16).await?;
				}
				writer.flush().await?;
				Ok(())
			}
			if let Err(e)=write_response(&mut writer,res).await{
				eprintln!("Response Network Send Error {}",e);
				break;
			}
		}
	}
	println!("End Connection");
}
async fn load_from_db(pool: &Pool<Postgres>,req:Request)->Response{
	fn single_string_body(req:&Request)->Result<String,Response>{
		let name=match String::from_utf8(req.body[2..].to_vec()){
			Ok(s)=>s,
			Err(_)=>return Err(Response{
				status:400,
				id:req.id,
				json:None,
			})
		};
		if name.is_empty() {
			Err(Response{
				status:400,
				id:req.id,
				json:None,
			})
		}else{
			Ok(name)
		}
	}
	let file = match req.request_type{
		1=>{
			let name=match single_string_body(&req){
				Ok(s)=>s,
				Err(e)=>return e,
			};
			sqlx::query_as::<_, DriveFile>(
				"
				SELECT \"thumbnailAccessKey\",\"webpublicAccessKey\",\"isLink\",src
				FROM drive_file
				WHERE \"webpublicAccessKey\" = $1
				",
			)
			.bind(&name)
			.fetch_one(pool).await
		},
		2=>{
			let name=match single_string_body(&req){
				Ok(s)=>s,
				Err(e)=>return e,
			};
			sqlx::query_as::<_, DriveFile>(
				"
				SELECT \"thumbnailAccessKey\",\"webpublicAccessKey\",\"isLink\",src
				FROM drive_file
				WHERE \"thumbnailAccessKey\" = $1
				",
			)
			.bind(&name)
			.fetch_one(pool).await
		}
		3=>{
			return Response{
				status:204,
				id:req.id,
				json:None,
			}
		},
		4=>{
			let name=match single_string_body(&req){
				Ok(s)=>s,
				Err(e)=>return e,
			};
			sqlx::query_as::<_, DriveFile>(
				"
				SELECT \"thumbnailAccessKey\",\"webpublicAccessKey\",\"isLink\",src
				FROM drive_file
				WHERE \"accessKey\" = $1
				",
			)
			.bind(&name)
			.fetch_one(pool).await
		},
		_=>{
			return Response{
				status:501,
				id:req.id,
				json:None,
			}
		}
	};
	println!("{:?}",file);
	match file{
		Ok(v)=>{
			let json:ResponseJson=v.into();
			Response{
				status:200,
				id:req.id,
				json:serde_json::to_string(&json).ok(),
			}
		},
		Err(e)=>{
			eprintln!("{}",e);
			match e{
				sqlx::Error::RowNotFound => Response{
					status:404,
					id:req.id,
					json:None
				},
				sqlx::Error::PoolClosed => Response{
					status:503,
					id:req.id,
					json:None
				},
				sqlx::Error::PoolTimedOut => Response{
					status:504,
					id:req.id,
					json:None
				},
				_ => Response{
					status:500,
					id:req.id,
					json:None
				},
			}
		}
	}
}
#[derive(Serialize,Deserialize,Debug)]
struct ResponseJson{
	uri:String,
	link:bool,
}
impl From<DriveFile> for ResponseJson{
	fn from(value: DriveFile) -> Self {
		Self{
			uri:value.src.unwrap_or_else(||String::new()),
			link:value.isLink,
		}
	}
}
