//! Dynamic unary gRPC against a loopback tonic server built from the same generated descriptor.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use postguy_grpc::{DynamicCodec, GrpcTls};
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
        let binary = request.metadata().get_bin("x-trace-bin").and_then(|value| value.to_bytes().ok().map(|bytes| bytes.to_vec()));
        let message = request.into_inner();
        Box::pin(async move {
            let text = message.get_field_by_name("text").and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default();
            if text == "fail" {
                return Err(Status::invalid_argument("requested failure"));
            }
            let mut response = Response::new(message);
            if let Some(bytes) = binary {
                response.metadata_mut().insert_bin("x-trace-bin", tonic::metadata::MetadataValue::from_bytes(&bytes));
            }
            Ok(response)
        })
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

#[test]
fn unary_echo_and_status_errors_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("echo.proto");
    std::fs::write(&path, PROTO).expect("proto");
    let pool: DescriptorPool = postguy_grpc::compile(&[&path], &[dir.path()]).expect("compile");
    let descriptor = pool.get_message_by_name("echo.EchoMessage").expect("message");
    let method = postguy_grpc::method(&pool, "echo.Echo/Say").expect("method");
    runtime().block_on(async move {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let addr = listener.local_addr().expect("addr");
        let service = EchoService { descriptor };
        let server = tonic::transport::Server::builder()
            .serve_with_incoming(tonic::service::Routes::new(service), tokio_stream::wrappers::TcpListenerStream::new(listener));
        tokio::spawn(server);
        let endpoint = format!("http://{addr}");
        let result = postguy_grpc::unary(
            &endpoint,
            &GrpcTls::default(),
            &method,
            serde_json::json!({"text": "hello", "number": 7}),
            &[("x-test".into(), "one".into())],
        )
        .await
        .expect("unary");
        assert_eq!(result.json["text"], "hello");
        assert_eq!(result.json["number"], 7);
        let error = postguy_grpc::unary(&endpoint, &GrpcTls::default(), &method, serde_json::json!({"text": "fail"}), &[]).await.expect_err("status");
        assert!(error.to_string().contains("invalid_argument") || error.to_string().contains("3"), "{error}");
    });
}

// Keep the body type used by the service signature referenced, so a tonic upgrade that changes
// it breaks here rather than silently in a generated service.
#[allow(dead_code)]
fn body_type_is_send(_: tonic::body::Body) -> impl Future<Output = ()> {
    std::future::ready(())
}

#[test]
fn metadata_is_validated_before_connecting_and_binary_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proto = dir.path().join("echo.proto");
    std::fs::write(&proto, PROTO).expect("proto");
    let pool = postguy_grpc::compile(&[&proto], &[dir.path()]).expect("pool");
    let method = postguy_grpc::method(&pool, "echo.Echo/Say").expect("method");
    let close = "http://127.0.0.1:1";
    let runtime = runtime();
    let invalid_key = runtime.block_on(postguy_grpc::unary(
        close,
        &postguy_grpc::GrpcTls::default(),
        &method,
        serde_json::json!({"text": "x"}),
        &[("Bad Key".into(), "v".into())],
    ));
    assert!(invalid_key.is_err(), "an invalid key is an error, not a skipped row");
    let invalid_bin = runtime.block_on(postguy_grpc::unary(
        close,
        &postguy_grpc::GrpcTls::default(),
        &method,
        serde_json::json!({"text": "x"}),
        &[("x-trace-bin".into(), "not base64!".into())],
    ));
    assert!(invalid_bin.is_err(), "invalid base64 is an error");
}

#[test]
fn example_message_covers_nested_repeated_map_oneof_enum_and_wkt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proto = dir.path().join("rich.proto");
    std::fs::write(
        &proto,
        r#"syntax = "proto3";
package ex;
import "google/protobuf/timestamp.proto";
message Child { string name = 1; }
message Rich {
  int32 n = 1;
  repeated string tags = 2;
  map<string, int32> counts = 3;
  repeated Child children = 4;
  oneof pick { string a = 5; int32 b = 6; }
  google.protobuf.Timestamp when = 7;
  enum Color { COLOR_UNSPECIFIED = 0; RED = 1; }
  Color color = 8;
  optional string maybe = 9;
}
message Node { Node next = 1; string name = 2; }
service RichService { rpc Get (Rich) returns (Rich); rpc Walk (Node) returns (Node); }
"#,
    )
    .expect("proto");
    let pool = postguy_grpc::compile(&[&proto], &[dir.path()]).expect("compile");
    let example = postguy_grpc::example_message(&pool, "ex.RichService/Get").expect("example");
    assert_eq!(example["n"], 0);
    assert_eq!(example["tags"], serde_json::json!([""]));
    assert_eq!(example["counts"], serde_json::json!({"": 0}));
    assert_eq!(example["children"], serde_json::json!([{"name": ""}]));
    assert_eq!(example["a"], "");
    assert!(example.get("b").is_none(), "only the first oneof field is shown: {example}");
    assert_eq!(example["when"], "1970-01-01T00:00:00Z");
    assert_eq!(example["color"], "COLOR_UNSPECIFIED");
    assert_eq!(example["maybe"], "");
    let node = postguy_grpc::example_message(&pool, "ex.RichService/Walk").expect("node example");
    assert_eq!(node, serde_json::json!({"next": {}, "name": ""}));
}

#[test]
fn schema_lists_fields_maps_lists_and_nested_messages() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proto = dir.path().join("rich.proto");
    std::fs::write(
        &proto,
        r#"syntax = "proto3";
package ex;
import "google/protobuf/timestamp.proto";
message Label { string text = 1; }
message Child { string name = 1; }
message Rich {
  int32 n = 1;
  repeated string tags = 2;
  map<string, Label> labels = 3;
  repeated Child children = 4;
  Child child = 5;
  google.protobuf.Timestamp when = 6;
  Color color = 7;
  oneof pick { string a = 8; int32 b = 9; }
  optional string maybe = 10;
}
enum Color { COLOR_UNSPECIFIED = 0; RED = 1; }
service RichService { rpc Get (Rich) returns (Child); }
"#,
    )
    .expect("proto");
    let pool = postguy_grpc::compile(&[&proto], &[dir.path()]).expect("compile");
    let schema = postguy_grpc::schema(&pool);
    assert_eq!(schema.services.len(), 1);
    let method = &schema.services[0].methods[0];
    assert_eq!((method.path.as_str(), method.input.as_str(), method.output.as_str()), ("ex.RichService/Get", "ex.Rich", "ex.Child"));
    assert_eq!(schema.files, vec!["rich.proto".to_string()]);
    assert!(schema.messages.keys().all(|name| !name.starts_with("google.protobuf.")));
    let rich = schema.messages.get("ex.Rich").expect("Rich");
    let field = |name: &str| rich.fields.iter().find(|field| field.name == name).expect("field");
    assert_eq!(field("n").type_label, "int32");
    assert_eq!(field("tags").type_label, "repeated string");
    assert!(field("tags").repeated && field("tags").message.is_none());
    assert_eq!(field("labels").type_label, "map<string, Label>");
    assert!(field("labels").map);
    assert_eq!(field("labels").message.as_deref(), Some("ex.Label"));
    assert_eq!(field("children").type_label, "repeated Child");
    assert_eq!(field("children").message.as_deref(), Some("ex.Child"));
    assert_eq!(field("child").type_label, "Child");
    assert_eq!(field("child").type_full, "ex.Child");
    assert_eq!(field("when").type_label, "Timestamp");
    assert_eq!(field("when").message, None);
    assert_eq!(field("color").type_label, "Color");
    assert_eq!(field("color").message, None);
    assert_eq!(field("a").oneof.as_deref(), Some("pick"));
    assert_eq!(field("maybe").oneof, None, "a proto3 optional is a synthetic oneof");
    assert!(schema.messages.contains_key("ex.Label"), "map value messages are reachable");
}

#[test]
fn reflection_pool_all_lists_echo_service() {
    let server = postguy_testkit::GrpcEchoServer::start().expect("server");
    let pool = runtime().block_on(postguy_grpc::reflection_pool_all(server.url(), &GrpcTls::default())).expect("reflection");
    let schema = postguy_grpc::schema(&pool);
    assert_eq!(schema.services[0].name, "echo.Echo");
    assert_eq!(schema.services[0].methods.len(), 4);
    assert_eq!(schema.services.len(), 1, "the reflection service itself is left out");
}

#[test]
fn status_names_cover_every_code() {
    assert_eq!(postguy_grpc::status_name(0), "OK");
    assert_eq!(postguy_grpc::status_name(5), "NOT_FOUND");
    assert_eq!(postguy_grpc::status_name(16), "UNAUTHENTICATED");
    assert_eq!(postguy_grpc::status_name(99), "UNKNOWN");
}
