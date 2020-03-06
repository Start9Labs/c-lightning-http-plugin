use std::future::Future;
use std::net::SocketAddr;

use failure::Error;
use futures::FutureExt;
use futures::TryStreamExt;
use hyper::body::Bytes;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server};
use lazy_async_pool::Pool;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use crate::async_io::RpcResponseStream;
use crate::async_io::TokioCompatAsyncRead;
use crate::lightning_socket::LightningSocketArc;

mod async_io;
mod lightning_socket;
mod rpc;

type BoxedByteStream = Box<
    dyn futures::Stream<Item = Result<Bytes, Box<dyn std::error::Error + 'static + Sync + Send>>>
        + 'static
        + Sync
        + Send,
>;

async fn handle_inner<
    F: Fn() -> U + Send + Sync + 'static,
    U: Future<Output = Result<UnixStream, E>> + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
>(
    pool: Pool<UnixStream, F, U, E>,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    match req.method() {
        &Method::POST => {
            let mut ustream = pool.get().await?;
            let body = req.into_body();
            tokio::io::copy(
                &mut TokioCompatAsyncRead(
                    body.map_err(|e| futures::io::Error::new(futures::io::ErrorKind::Other, e))
                        .into_async_read(),
                ),
                &mut *ustream,
            )
            .await?;
            ustream.write_all(b"\n\n").await?;
            let stream = RpcResponseStream::new(ustream)
            .map_err(|e| -> Box<dyn std::error::Error + 'static + Sync + Send> {
                Box::new(e)
            })
            .into_stream();
            let res: BoxedByteStream = Box::new(
                stream,
            );
            Response::builder().header("Content-Type", "application/json").body(Body::from(res)).map_err(Error::from)
        }
        _ => Response::builder()
            .header("Content-Type", "application/json")
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::from("{\"id\":null,\"jsonrpc\":\"2.0\",\"error\":{\"code\":4,\"message\":\"Method Not Allowed\"}}"))
            .map_err(Error::from),
    }
}

async fn handle<
    F: Fn() -> U + Send + Sync + 'static,
    U: Future<Output = Result<UnixStream, E>> + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
>(
    pool: Pool<UnixStream, F, U, E>,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    match handle_inner(pool, req).await {
        Err(e) => Response::builder()
            .header("Content-Type", "application/json")
            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(serde_json::to_string(&crate::rpc::RpcRes {
                id: crate::rpc::JsonRpcV2Id::Null,
                jsonrpc: Default::default(),
                result: crate::rpc::RpcResult::Error(crate::rpc::RpcError {
                    code: serde_json::Number::from(5),
                    message: "internal server error",
                    data: Some(serde_json::Value::String(format!("{}", e))),
                }),
            })?))
            .map_err(Error::from),
        a => a,
    }
}

#[tokio::main]
async fn main() {
    // Construct our SocketAddr to listen on...
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let (send_side, recv_side) = crossbeam_channel::bounded(1);

    // fork thread for stdio
    let lightning_socket_fut = LightningSocketArc::new(recv_side);

    let pool = Pool::new(0, move || {
        let lightning_socket_fut = lightning_socket_fut.clone();
        async move { UnixStream::connect(&*lightning_socket_fut.wait_for_path().await).await }
            .boxed()
    });
    // And a MakeService to handle each connection...
    let handler = move |req| handle((&pool).clone(), req);
    let make_service = make_service_fn(|_conn| {
        let handler = handler.clone();
        futures::future::ok::<_, Error>(service_fn(handler))
    });

    // Then bind and serve...
    let server = Server::bind(&addr).serve(make_service);

    std::thread::spawn(move || {
        crate::rpc::handle_stdio_rpc(send_side);
    });

    // And run forever...
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
