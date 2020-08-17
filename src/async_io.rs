use std::ops::DerefMut;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use futures::Stream;
use hyper::body::Bytes;
use tokio::io::AsyncRead;
use tokio::io::Result as TokioResult;

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
pub enum RpcResponseState {
    NoNewLines,
    OneNewLine,
    TwoNewLines,
}

/// A byte stream that terminates when 2 consecutive newlines are received
pub struct RpcResponseStream<SP: DerefMut<Target = AR>, AR: AsyncRead, F: FnOnce(&mut SP) -> ()> {
    pub inner: SP,
    pub state: RpcResponseState,
    pub buf: [u8; 4096],
    pub finalizer: Option<F>,
}
impl<SP, AR, F> RpcResponseStream<SP, AR, F>
where
    SP: DerefMut<Target = AR>,
    AR: AsyncRead,
    F: FnOnce(&mut SP) -> (),
{
    pub fn new(sp: SP, finalizer: Option<F>) -> Self {
        RpcResponseStream {
            inner: sp,
            state: RpcResponseState::NoNewLines,
            buf: [0; 4096],
            finalizer,
        }
    }
}
impl<SP, AR, F> Stream for RpcResponseStream<SP, AR, F>
where
    SP: DerefMut<Target = AR> + std::marker::Unpin,
    AR: AsyncRead + std::marker::Unpin,
    F: FnOnce(&mut SP) -> () + std::marker::Unpin,
{
    type Item = Result<Bytes, tokio::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let mut buf = self.buf;
        if self.state == RpcResponseState::TwoNewLines {
            if let Some(finalizer) = self.finalizer.take() {
                finalizer(&mut self.inner);
            }
            return Poll::Ready(None);
        }
        let inner_pin: Pin<&mut AR> = Pin::new(self.inner.deref_mut());
        let bytes_read_poll = AsyncRead::poll_read(inner_pin, cx, &mut buf);
        let state = &mut self.state;
        let res = match bytes_read_poll {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(n)) => {
                let mut buf = &buf[..n];
                if buf.ends_with(b"\n\n") {
                    *state = RpcResponseState::TwoNewLines;
                    buf = &buf[..n - 2];
                } else if *state == RpcResponseState::OneNewLine && buf.starts_with(b"\n") {
                    buf = &buf[..n - 1];
                } else if buf.ends_with(b"\n") {
                    *state = RpcResponseState::OneNewLine;
                    buf = &buf[..n - 1];
                } else {
                    *state = RpcResponseState::NoNewLines;
                }
                Poll::Ready(Some(Ok(Bytes::from(buf.to_owned()))))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
        };
        self.buf = buf;
        res
    }
}
