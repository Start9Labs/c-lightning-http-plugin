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
                    self.state = RPCResponseState::OneNewLine;
                } else {
                    self.state = RPCResponseState::NoNewLines;
                }
                Poll::Ready(Some(Ok(Bytes::from(buf))))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
        }
    }
}
