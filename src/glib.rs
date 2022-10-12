//! Optional module containing a [`MainThreadContext`](crate::MainThreadContext) implementation for
//! gtk-rs [`glib`](https://gtk-rs.org/).

#![cfg_attr(docsrs, doc(cfg(feature = "glib")))]

/// A context for running a JACK client on a [`glib::MainContext`].
///
/// Should be used with [`ClientBuilder`](crate::ClientBuilder).
#[derive(Debug)]
#[repr(transparent)]
pub struct GlibContext {
    context: glib::MainContext,
}

impl GlibContext {
    /// Creates a `GlibContext` wrapping a given [`glib::MainContext`].
    #[inline]
    pub fn new(context: glib::MainContext) -> Self {
        Self { context }
    }
}

impl Default for GlibContext {
    #[inline]
    fn default() -> Self {
        Self {
            context: glib::MainContext::ref_thread_default(),
        }
    }
}

impl crate::MainThreadContext for GlibContext {
    #[inline]
    fn spawn_local<F: std::future::Future<Output = ()> + 'static>(&self, fut: F) {
        self.context.spawn_local(fut);
    }
    type IntervalStream = std::pin::Pin<Box<dyn futures_core::Stream<Item = ()> + Send + 'static>>;
    #[inline]
    fn interval(&self, period: std::time::Duration) -> Self::IntervalStream {
        glib::interval_stream(period)
    }
}
