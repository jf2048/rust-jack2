use futures_util::StreamExt;
use tokio_stream::wrappers::IntervalStream;

#[derive(Debug)]
#[repr(transparent)]
pub struct TokioIntervalStream(IntervalStream);

impl futures_core::Stream for TokioIntervalStream {
    type Item = ();

    #[inline]
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_next_unpin(cx).map(|o| o.map(|_| ()))
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

#[derive(Debug, Default)]
pub struct TokioContext;

impl crate::MainThreadContext for TokioContext {
    #[inline]
    fn spawn_local<F: std::future::Future<Output = ()> + 'static>(&self, fut: F) {
        tokio::task::spawn_local(fut);
    }
    type IntervalStream = TokioIntervalStream;
    #[inline]
    fn interval(&self, period: std::time::Duration) -> Self::IntervalStream {
        TokioIntervalStream(IntervalStream::new(tokio::time::interval(period)))
    }
}
