use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::stream::Stream;
use tokio::sync::watch;

pub struct AbortHandle(watch::Sender<()>);

#[derive(Clone)]
pub struct AbortableStreamFactory(watch::Receiver<()>);

pub enum AbortStreamError {
    Aborted,
}

pub enum AbortTryStreamError<E> {
    Aborted(AbortStreamError),
    Err(E),
}

pub struct AbortableTryStream<S, T, E>
where
    S: Stream<Item = Result<T, E>> + Unpin,
{
    aborter: watch::Receiver<()>,
    stream: S,
}

impl<S: Stream<Item = Result<T, E>> + Unpin, T, E> Stream for AbortableTryStream<S, T, E> {
    type Item = Result<T, AbortTryStreamError<E>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.aborter).poll_next(cx) {
            Poll::Ready(Some(())) | Poll::Pending => (),
            Poll::Ready(None) => {
                return Poll::Ready(Some(Err(AbortTryStreamError::Aborted(
                    AbortStreamError::Aborted,
                ))))
            }
        }

        match Pin::new(&mut self.stream).poll_next(cx) {
            Poll::Ready(Some(Ok(v))) => Poll::Ready(Some(Ok(v))),
            Poll::Ready(Some(Err(v))) => Poll::Ready(Some(Err(AbortTryStreamError::Err(v)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct AbortableStream<S>
where
    S: Stream + Unpin,
{
    aborter: watch::Receiver<()>,
    stream: S,
}

impl<S: Stream + Unpin> Stream for AbortableStream<S> {
    type Item = Result<S::Item, AbortStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        match Stream::poll_next(Pin::new(&mut self.aborter), cx) {
            Poll::Ready(Some(())) | Poll::Pending => (),
            Poll::Ready(None) => return Poll::Ready(Some(Err(AbortStreamError::Aborted))),
        }

        match Stream::poll_next(Pin::new(&mut self.stream), cx) {
            Poll::Ready(Some(v)) => Poll::Ready(Some(Ok(v))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub fn abortable_stream_pair() -> (AbortHandle, AbortableStreamFactory) {
    let (tx, rx) = watch::channel(());
    (AbortHandle(tx), AbortableStreamFactory(rx))
}

impl AbortableStreamFactory {
    pub fn with_stream<S>(&self, stream: S) -> AbortableStream<S>
    where
        S: Stream + Unpin,
    {
        AbortableStream {
            aborter: self.0.clone(),
            stream,
        }
    }

    pub fn with_try_stream<S, T, E>(&self, stream: S) -> AbortableTryStream<S, T, E>
    where
        S: Stream<Item = Result<T, E>> + Unpin,
    {
        AbortableTryStream {
            aborter: self.0.clone(),
            stream,
        }
    }
}

pub struct SinkCloser<T, S>
where
    S: tokio::prelude::Sink<T> + Unpin,
{
    sink: S,
    marker: PhantomData<T>,
}

impl<T, S> Unpin for SinkCloser<T, S> where S: tokio::prelude::Sink<T> + Unpin {}

impl<T, S> SinkCloser<T, S>
where
    S: tokio::prelude::Sink<T> + Unpin,
{
    pub fn new(sink: S) -> SinkCloser<T, S> {
        SinkCloser {
            sink: sink,
            marker: Default::default(),
        }
    }
}

impl<T, S> std::future::Future for SinkCloser<T, S>
where
    S: tokio::prelude::Sink<T> + Unpin,
{
    type Output = Result<(), S::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        tokio::prelude::Sink::poll_close(Pin::new(&mut self.sink), cx)
    }
}
