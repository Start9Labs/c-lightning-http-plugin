use std::future::Future;
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use failure::Error;
use futures::FutureExt;
use futures::Stream;
use futures::TryStreamExt;
use hyper::body::Bytes;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server};
use lazy_async_pool::Pool;
use tokio::io::AsyncRead;
use tokio::io::Result as TokioResult;
use tokio::net::UnixStream;

type BoxedByteStream = Box<
    dyn futures::Stream<Item = Result<Bytes, Box<dyn std::error::Error + 'static + Sync + Send>>>
        + 'static
        + Sync
        + Send,
>;

/// A small wrapper type to implement tokio::AsyncRead for things that implement futures::AsyncRead
pub struct TokioCompatAsyncRead<AR: futures::AsyncRead>(pub AR);
impl<AR> AsyncRead for TokioCompatAsyncRead<AR>
where
    AR: futures::AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<TokioResult<usize>> {
        futures::AsyncRead::poll_read(unsafe { self.map_unchecked_mut(|a| &mut a.0) }, cx, buf)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RPCResponseState {
    NoNewLines,
    OneNewLine,
    TwoNewLines,
}

/// A byte stream that terminates when 2 consecutive newlines are received
pub struct RPCResponseStream<SP: DerefMut<Target = AR>, AR: AsyncRead> {
    pub inner: SP,
    pub state: RPCResponseState,
}
impl<SP, AR> RPCResponseStream<SP, AR>
where
    SP: DerefMut<Target = AR>,
    AR: AsyncRead,
{
    pub fn new(sp: SP) -> Self {
        RPCResponseStream {
            inner: sp,
            state: RPCResponseState::NoNewLines,
        }
    }
}
impl<SP, AR> Stream for RPCResponseStream<SP, AR>
where
    SP: DerefMut<Target = AR> + std::marker::Unpin,
    AR: AsyncRead + std::marker::Unpin,
{
    type Item = Result<Bytes, tokio::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        if self.state == RPCResponseState::TwoNewLines {
            return Poll::Ready(None);
        }
        let mut buf = vec![0; 4096];
        let inner_pin: Pin<&mut AR> = Pin::new(self.inner.deref_mut());
        let bytes_read_poll = AsyncRead::poll_read(inner_pin, cx, &mut buf);
        let state = &mut self.state;
        match bytes_read_poll {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(n)) => {
                buf.truncate(n);
                if buf.ends_with(b"\n\n")
                    || (*state == RPCResponseState::OneNewLine && buf.starts_with(b"\n"))
                {
                    *state = RPCResponseState::TwoNewLines;
                } else if buf.ends_with(b"\n") {
                    unsafe {
                        self.get_unchecked_mut().state = RPCResponseState::OneNewLine;
                    }
                } else {
                    unsafe {
                        self.get_unchecked_mut().state = RPCResponseState::NoNewLines;
                    }
                }
                Poll::Ready(Some(Ok(Bytes::from(buf))))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
        }
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
            let res: BoxedByteStream = Box::new(
                RPCResponseStream::new(ustream)
                    .map_err(|e| -> Box<dyn std::error::Error + 'static + Sync + Send> {
                        Box::new(e)
                    })
                    .into_stream(),
            );
            Ok(Response::new(Body::from(res)))
        }
        _ => Response::builder()
            .header("Content-Type", "application/json")
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::from("{\"error\":\"Method Not Allowed\"}"))
            .map_err(Error::from),
    }
}

#[tokio::main]
async fn main() {
    // Construct our SocketAddr to listen on...
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let pool = Pool::new(0, || UnixStream::connect("TODO").boxed());
    // And a MakeService to handle each connection...
    let handler = move |req| handle((&pool).clone(), req);
    let make_service = make_service_fn(|_conn| {
        let handler = handler.clone();
        futures::future::ok::<_, Error>(service_fn(handler))
    });

    // Then bind and serve...
    let server = Server::bind(&addr).serve(make_service);

    // And run forever...
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
