use linkerd_stack as svc;
use linkerd_tls as tls;
use std::io;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tls::NegotiatedProtocolRef;
use tokio_rustls::{
    client::{Connect, TlsStream},
    rustls::{self, ClientConfig, Session},
};

// pub type Io<I> = io::EitherIo<I, TlsStream<I>>;
// type Connect<F, I> = MapOk<F, fn(I) -> Io<I>>;
// type Handshake<I> = Pin<Box<dyn Future<Output = io::Result<Io<I>>> + Send + 'static>>;

pub struct InitTls {
    connect: tokio_rustls::TlsConnector,
}

impl svc::Service<(tls::ServerId, I)> for InitTls
where
    T: io::AsyncRead + io::AsyncWrite + Send + Unpin,
{
    type Response = (Option<tls::NegotiatedProtocol>, TlsStream<I>);
    type Error = io::Error;
    type Future = futures::future::MapOk<
        Connect<I>,
        fn(TlsStream<I>) -> (Option<tls::NegotiatedProtocol>, TlsStream<I>),
    >;

    #[inline]
    fn poll_ready(&self, cx: Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&self, (server_id, tcp): (tls::ServerId, I)) -> Poll<io::Result<()>> {
        self.connect
            .connect(server_id.as_ref().into(), tcp)
            .map_ok(|tls| {
                let alpn = {
                    let (_, sess) = tls.get_ref();
                    sess.get_alpn_protocol()
                        .map(|p| tls::NegotiatedProtocolRef(p).into())
                };
                (alpn, tls)
            })
    }
}
