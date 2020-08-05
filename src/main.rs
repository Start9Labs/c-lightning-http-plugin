use std::future::Future;
use std::net::SocketAddr;

use failure::Error;
use futures::FutureExt;
use futures::TryStreamExt;
use hyper::body::Bytes;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server};
use lazy_async_pool::{Pool, PoolGuard};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use crate::async_io::RpcResponseStream;
use crate::async_io::TokioCompatAsyncRead;
use crate::init_info::InitInfoArc;

mod async_io;
mod init_info;
mod rpc;

type BoxedByteStream = Box<
    dyn futures::Stream<Item = Result<Bytes, Box<dyn std::error::Error + 'static + Sync + Send>>>
        + 'static
        + Send,
>;

async fn handle_auth(
    init_info_fut: InitInfoArc,
    auth: Option<&hyper::header::HeaderValue>,
) -> bool {
    if let (Some(received), Some(expected)) =
        (auth, &init_info_fut.wait_for_info().await.auth_header)
    {
        received == expected
    } else {
        false
    }
}

async fn handle_inner<
    F: Fn() -> U + Send + Sync + 'static,
    U: Future<Output = Result<UnixStream, E>> + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
>(
    pool: Pool<UnixStream, F, U, E>,
    init_info_fut: InitInfoArc,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    match req.method() {
        &Method::POST => {
            if !handle_auth(init_info_fut, req.headers().get("Authorization")).await {
                return Response::builder().header("Content-Type", "application/json")
                    .status(hyper::StatusCode::UNAUTHORIZED)
                    .body(Body::from("{\"id\":null,\"jsonrpc\":\"2.0\",\"error\":{\"code\":5,\"message\":\"Unauthorized\"}}"))
                    .map_err(Error::from);
            }
            let mut ustream = pool.get().await?;
            let body = req.into_body();
            ustream.mark_dirty();
            tokio::io::copy(
                &mut TokioCompatAsyncRead(
                    body.map_err(|e| futures::io::Error::new(futures::io::ErrorKind::Other, e))
                        .into_async_read(),
                ),
                &mut *ustream,
            )
            .await?;
            ustream.write_all(b"\n\n").await?;
            let stream = RpcResponseStream::new(ustream, Some(|s: &mut PoolGuard<UnixStream, _, _, _>| s.mark_clean()))
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
    init_info_fut: InitInfoArc,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    match handle_inner(pool, init_info_fut, req).await {
        Err(e) => Response::builder()
            .header("Content-Type", "application/json")
            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(serde_json::to_string(&crate::rpc::RpcRes {
                id: crate::rpc::JsonRpcV2Id::Null,
                jsonrpc: Default::default(),
                result: crate::rpc::RpcResult::Error(crate::rpc::RpcError {
                    code: serde_json::Number::from(0),
                    message: "Internal Server Error",
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
    let (send_side, recv_side) = crossbeam_channel::bounded(1);

    std::thread::spawn(move || {
        crate::rpc::handle_stdio_rpc(send_side);
    });

    // fork thread for stdio
    let init_info_fut = InitInfoArc::new(recv_side);

    let init_info_fut_cap = init_info_fut.clone();
    let pool = Pool::new(0, move || {
        let init_info_fut = init_info_fut_cap.clone();
        async move { UnixStream::connect(&*init_info_fut.wait_for_info().await.socket_path).await }
            .boxed()
    });
    // And a MakeService to handle each connection...
    let init_info_fut_cap = init_info_fut.clone();
    let handler = move |req| handle((&pool).clone(), init_info_fut_cap.clone(), req);
    let make_service = make_service_fn(|_conn| {
        let handler = handler.clone();
        futures::future::ok::<_, Error>(service_fn(handler))
    });

    let port = init_info_fut.wait_for_info().await.http_port;

    // Then bind and serve...
    let server = Server::bind(&SocketAddr::from(([127, 0, 0, 1], port))).serve(make_service);

    eprintln!("Serving RPC on port {}", port);

    // And run forever...
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
