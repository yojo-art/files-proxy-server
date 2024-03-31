set -eu
cd $(dirname $0)
cd server
cargo build --target x86_64-unknown-linux-musl --release
cd ..

#ここにAgentのビルド処理

mkdir -p bin
cp -a server/target/x86_64-unknown-linux-musl/release/server bin/server_x86_64

