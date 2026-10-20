//! A loopback gRPC echo server for tool/engine tests. The proto is contributor-written in this
//! file (no committed sample asset) and compiled with protox at start.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use postguy_grpc::DynamicCodec;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use tokio::net::TcpListener;
use tonic::server::{Grpc, NamedService, UnaryService};
use tonic::{Request, Response, Status};

const PROTO: &str = r#"
syntax = "proto3";
package echo;
service Echo { rpc Say (EchoMessage) returns (EchoMessage); }
message EchoMessage { string text = 1; int32 number = 2; }
"#;

/// The echo server's endpoint and descriptor (tests assert against it).
pub struct GrpcEchoServer {
    url: String,
    pool: DescriptorPool,
    _dir: tempfile::TempDir,
}

impl GrpcEchoServer {
    pub fn start() -> Result<GrpcEchoServer, String> {
        let dir = tempfile::tempdir().map_err(|err| format!("grpc tempdir: {err}"))?;
        let proto = dir.path().join("echo.proto");
        std::fs::write(&proto, PROTO).map_err(|err| format!("writing echo.proto: {err}"))?;
        let set = postguy_grpc::compile_set(&[&proto], &[dir.path()]).map_err(|err| err.to_string())?;
        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(set.clone()).map_err(|err| err.to_string())?;
        let mut encoded = Vec::new();
        prost::Message::encode(&set, &mut encoded).map_err(|err| err.to_string())?;
        let reflection = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(&encoded)
            .build_v1()
            .map_err(|err| err.to_string())?;
        let descriptor = pool.get_message_by_name("echo.EchoMessage").ok_or("missing EchoMessage descriptor")?;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("binding grpc server: {err}"))?;
        let addr = listener.local_addr().map_err(|err| format!("grpc address: {err}"))?;
        listener.set_nonblocking(true).map_err(|err| format!("grpc nonblocking: {err}"))?;
        let service = EchoService { descriptor };
        std::thread::Builder::new()
            .name("testkit-grpc".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                runtime.block_on(async move {
                    let Ok(listener) = TcpListener::from_std(listener) else { return };
                    let server = tonic::transport::Server::builder().serve_with_incoming(
                        tonic::service::Routes::new(service).add_service(reflection),
                        tokio_stream::wrappers::TcpListenerStream::new(listener),
                    );
                    let _ = server.await;
                });
            })
            .map_err(|err| format!("starting grpc server: {err}"))?;
        Ok(GrpcEchoServer { url: format!("http://{addr}"), pool, _dir: dir })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn pool(&self) -> &DescriptorPool {
        &self.pool
    }
}

#[derive(Clone)]
struct EchoService {
    descriptor: MessageDescriptor,
}

impl NamedService for EchoService {
    const NAME: &'static str = "echo.Echo";
}

impl tower::Service<http::Request<tonic::body::Body>> for EchoService {
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<tonic::body::Body>) -> Self::Future {
        let descriptor = self.descriptor.clone();
        Box::pin(async move {
            let mut grpc = Grpc::new(DynamicCodec::new(&descriptor));
            Ok(grpc.unary(EchoCall, request).await)
        })
    }
}

struct EchoCall;

impl UnaryService<DynamicMessage> for EchoCall {
    type Response = DynamicMessage;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Self::Response>, Status>> + Send>>;

    fn call(&mut self, request: Request<DynamicMessage>) -> Self::Future {
        let message = request.into_inner();
        Box::pin(async move {
            let text = message.get_field_by_name("text").and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default();
            if text == "fail" {
                return Err(Status::invalid_argument("requested failure"));
            }
            Ok(Response::new(message))
        })
    }
}
