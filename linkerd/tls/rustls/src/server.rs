use linkerd_io as io;
use linkerd_stack as svc;
use linkerd_tls as tls;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio_rustls::{rustls, server::TlsStream};
use tracing::debug;

pub type Config = std::sync::Arc<rustls::ServerConfig>;

pub struct TerminateTls {
    config: Config,
}

impl<I> svc::Service<I> for TerminateTls
where
    T: io::AsyncRead + io::AsyncWrite + Send + Unpin,
{
    type Response = (tls::ServerTls, TlsStream<I>);
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send + 'static>>;

    #[inline]
    fn poll_ready(&self, cx: Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&self, io: I) -> Poll<io::Result<()>> {
        let config = self.config.clone();
        Box::pin(handshake(config, io))
    }
}

async fn handshake<T>(tls_config: Config, io: T) -> io::Result<(ServerTls, TlsStream<T>)>
where
    T: io::AsyncRead + io::AsyncWrite + Unpin,
{
    let io = tokio_rustls::TlsAcceptor::from(tls_config)
        .accept(io)
        .await?;

    // Determine the peer's identity, if it exist.
    let client_id = client_identity(&io);

    let negotiated_protocol = io
        .get_ref()
        .1
        .get_alpn_protocol()
        .map(|b| NegotiatedProtocol(b.into()));

    debug!(client.id = ?client_id, alpn = ?negotiated_protocol, "Accepted TLS connection");
    let tls = ServerTls::Established {
        client_id,
        negotiated_protocol,
    };
    Ok((tls, io))
}

fn client_identity<S>(tls: &TlsStream<S>) -> Option<ClientId> {
    use webpki::GeneralDNSNameRef;

    let (_io, session) = tls.get_ref();
    let certs = session.get_peer_certificates()?;
    let c = certs.first().map(rustls::Certificate::as_ref)?;
    let end_cert = webpki::EndEntityCert::from(c).ok()?;
    let dns_names = end_cert.dns_names().ok()?;

    match dns_names.first()? {
        GeneralDNSNameRef::DNSName(n) => {
            Some(ClientId(id::Name::from(dns::Name::from(n.to_owned()))))
        }
        GeneralDNSNameRef::Wildcard(_) => {
            // Wildcards can perhaps be handled in a future path...
            None
        }
    }
}
