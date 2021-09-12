use crate::{
    self as http,
    client_handle::{Closed, SetClientHandle},
    glue::{HyperServerSvc, UpgradeBody},
    h2::Settings as H2Settings,
    trace, upgrade, Version,
};
use futures::future;
use linkerd_error::{Error, Result};
use linkerd_io::{self as io, PeerAddr};
use linkerd_stack::{layer, NewService, Param};
use std::task::{Context, Poll};
use tower::Service;
use tracing::debug;

type Server = hyper::server::conn::Http<trace::Executor>;

#[derive(Clone, Debug)]
pub struct NewServeHttp<N> {
    inner: N,
    server: Server,
    drain: drain::Watch,
}

#[derive(Clone, Debug)]
pub struct ServeHttp<S> {
    version: Version,
    server: Server,
    inner: S,
    drain: drain::Watch,
}

// === impl NewServeHttp ===

impl<N> NewServeHttp<N> {
    pub fn layer(
        h2: H2Settings,
        drain: drain::Watch,
    ) -> impl layer::Layer<N, Service = Self> + Clone {
        layer::mk(move |inner| Self::new(h2, inner, drain.clone()))
    }

    /// Creates a new `ServeHttp`.
    fn new(h2: H2Settings, inner: N, drain: drain::Watch) -> Self {
        let mut server = hyper::server::conn::Http::new().with_executor(trace::Executor::new());
        server
            .max_buf_size(16 * 1024)
            .http2_initial_stream_window_size(h2.initial_stream_window_size)
            .http2_initial_connection_window_size(h2.initial_connection_window_size);

        // Configure HTTP/2 PING frames
        if let Some(timeout) = h2.keepalive_timeout {
            // XXX(eliza): is this a reasonable interval between
            // PING frames?
            let interval = timeout / 4;
            server
                .http2_keep_alive_timeout(timeout)
                .http2_keep_alive_interval(interval);
        }

        Self {
            inner,
            server,
            drain,
        }
    }
}

impl<T, N> NewService<T> for NewServeHttp<N>
where
    T: Param<Version>,
    N: NewService<T> + Clone,
{
    type Service = ServeHttp<N::Service>;

    fn new_service(&self, target: T) -> Self::Service {
        let version = target.param();
        debug!(?version, "Creating HTTP service");
        let inner = self.inner.new_service(target);
        ServeHttp {
            inner,
            version,
            server: self.server.clone(),
            drain: self.drain.clone(),
        }
    }
}

// === impl ServeHttp ===

impl<S> ServeHttp<S>
where
    S: Service<http::Request<UpgradeBody>, Response = http::Response<http::BoxBody>, Error = Error>,
    S: Clone + Unpin + Send + 'static,
    S::Future: Send + 'static,
{
    async fn serve_http1<I>(
        io: I,
        mut server: Server,
        svc: SetClientHandle<S>,
        drain: drain::Watch,
        closed: Closed,
    ) -> Result<()>
    where
        I: io::AsyncRead + io::AsyncWrite + PeerAddr + Send + Unpin + 'static,
    {
        // Enable support for HTTP upgrades (CONNECT and websockets).
        let mut conn = server
            .http1_only(true)
            .serve_connection(io, upgrade::Service::new(svc, drain.clone()))
            .with_upgrades();

        tokio::select! {
            res = &mut conn => {
                debug!(?res, "The client is shutting down the connection");
                res?
            }
            shutdown = drain.signaled() => {
                debug!("The process is shutting down the connection");
                Pin::new(&mut conn).graceful_shutdown();
                shutdown.release_after(conn).await?;
            }
            () = closed => {
                debug!("The stack is tearing down the connection");
                Pin::new(&mut conn).graceful_shutdown();
                conn.await?;
            }
        }

        Ok(())
    }

    async fn serve_h2<I>(
        io: I,
        mut server: Server,
        svc: SetClientHandle<S>,
        drain: drain::Watch,
        closed: Closed,
    ) -> Result<()>
    where
        I: io::AsyncRead + io::AsyncWrite + PeerAddr + Send + Unpin + 'static,
    {
        let mut conn = server
            .http2_only(true)
            .serve_connection(io, HyperServerSvc::new(svc));

        tokio::select! {
            res = &mut conn => {
                debug!(?res, "The client is shutting down the connection");
                res?
            }
            shutdown = drain.signaled() => {
                debug!("The process is shutting down the connection");
                Pin::new(&mut conn).graceful_shutdown();
                shutdown.release_after(conn).await?;
            }
            () = closed => {
                debug!("The stack is tearing down the connection");
                Pin::new(&mut conn).graceful_shutdown();
                conn.await?;
            }
        }

        Ok(())
    }
}

impl<I, S> Service<I> for ServeHttp<S>
where
    I: io::AsyncRead + io::AsyncWrite + PeerAddr + Send + Unpin + 'static,
    S: Service<http::Request<UpgradeBody>, Response = http::Response<http::BoxBody>, Error = Error>,
    S: Clone + Unpin + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = ();
    type Error = Error;
    type Future = future::BoxFuture<'static, Result<()>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, io: I) -> Self::Future {
        let Self {
            version,
            inner,
            drain,
            server,
        } = self.clone();
        debug!(?version, "Handling as HTTP");

        let (svc, closed) = match io.peer_addr() {
            Ok(pa) => SetClientHandle::new(pa, inner),
            Err(e) => return Box::pin(future::err(e.into())),
        };

        match version {
            Version::Http1 => Box::pin(Self::serve_http1(io, server, svc, drain, closed)),
            Version::H2 => Box::pin(Self::serve_h2(io, server, svc, drain, closed)),
        }
    }
}
