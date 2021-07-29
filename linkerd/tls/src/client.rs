#![allow(clippy::type_complexity)]

use crate::NegotiatedProtocol;
use futures::{
    future::{Either, MapOk},
    prelude::*,
};
use linkerd_conditional::Conditional;
use linkerd_dns_name::{InvalidName, Name};
use linkerd_io as io;
use linkerd_stack::{self as svc, Param, ServiceExt};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};
use tracing::{debug, trace};

/// A newtype for target server identities.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ServerId(pub Name);

/// A stack paramter that configures a `Client` to establish a TLS connection.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ClientTls {
    pub server_id: ServerId,
    pub alpn: Option<AlpnProtocols>,
}

/// A stack param that configures the available ALPN protocols.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct AlpnProtocols(pub Vec<Vec<u8>>);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum NoClientTls {
    /// Identity is administratively disabled.
    Disabled,

    /// No TLS is wanted because the connection is a loopback connection which
    /// doesn't need or support TLS.
    Loopback,

    /// The destination service didn't give us the identity, which is its way
    /// of telling us that we shouldn't do TLS for this endpoint.
    NotProvidedByServiceDiscovery,

    /// No discovery was attempted.
    IngressWithoutOverride,
}

/// A stack paramater that indicates whether the target server endpoint has a
/// known TLS identity.
pub type ConditionalClientTls = Conditional<ClientTls, NoClientTls>;

#[derive(Clone, Debug)]
pub struct Client<L, C> {
    tls: Option<L>,
    inner: C,
}

type Connected<I> = (Option<NegotiatedProtocol>, I);
type Connect<F, I, J> = MapOk<F, fn(I) -> Connected<J>>;
type Handshake<I> = Pin<Box<dyn Future<Output = io::Result<Connected<I>>> + Send + 'static>>;

// === impl Client ===

impl<L: Clone, C> Client<L, C> {
    pub fn layer(tls: Option<L>) -> impl svc::layer::Layer<C, Service = Self> + Clone {
        svc::layer::mk(move |inner| Self {
            inner,
            tls: tls.clone(),
        })
    }
}

impl<T, L, LIo, C> svc::Service<T> for Client<L, C>
where
    T: Param<ConditionalClientTls>,
    C: svc::Service<T, Error = io::Error>,
    C::Response: io::AsyncRead + io::AsyncWrite + Send + Unpin,
    C::Future: Send + 'static,
    L: svc::Service<(ClientTls, C::Response), Response = Connected<LIo>, Error = io::Error>
        + Clone
        + Send
        + 'static,
    L::Future: Send + 'static,
    LIo: io::AsyncRead + io::AsyncWrite + Send + Unpin,
{
    type Response = Connected<io::EitherIo<C::Response, LIo>>;
    type Error = io::Error;
    type Future = Either<
        Connect<C::Future, C::Response, io::EitherIo<C::Response, LIo>>,
        Handshake<io::EitherIo<C::Response, LIo>>,
    >;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, target: T) -> Self::Future {
        let client_tls = target.param();
        let connect = self.inner.call(target);

        let client_tls = match client_tls {
            Conditional::Some(client_tls) => client_tls,
            Conditional::None(reason) => {
                debug!(%reason, "Peer does not support TLS");
                return Either::Left(connect.map_ok(|io| (None, io::EitherIo::Left(io))));
            }
        };

        let tls = match self.tls.clone() {
            Some(tls) => tls,
            None => {
                trace!("Local identity disabled");
                return Either::Left(connect.map_ok(|io| (None, io::EitherIo::Left(io))));
            }
        };

        debug!(server.id = %client_tls.server_id, "Initiating TLS connection");
        Either::Right(Box::pin(async move {
            let tcp = connect.await?;
            let (alpn, tls): Connected<LIo> = tls.oneshot((client_tls, tcp)).await?;
            if let Some(alpn) = alpn.as_ref() {
                debug!(?alpn);
            }
            let conn: Connected<io::EitherIo<C::Response, LIo>> = (alpn, io::EitherIo::Right(tls));
            Ok(conn)
        }))
    }
}

// === impl ServerId ===

impl From<Name> for ServerId {
    fn from(n: Name) -> Self {
        Self(n)
    }
}

impl From<ServerId> for Name {
    fn from(ServerId(name): ServerId) -> Name {
        name
    }
}

impl AsRef<Name> for ServerId {
    fn as_ref(&self) -> &Name {
        &self.0
    }
}

impl FromStr for ServerId {
    type Err = InvalidName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Name::from_str(s).map(ServerId)
    }
}

impl fmt::Display for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

// === impl NoClientTls ===

impl fmt::Display for NoClientTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "disabled"),
            Self::Loopback => write!(f, "loopback"),
            Self::NotProvidedByServiceDiscovery => {
                write!(f, "not_provided_by_service_discovery")
            }
            Self::IngressWithoutOverride => {
                write!(f, "ingress_without_override")
            }
        }
    }
}

// === impl AlpnProtocols ===

impl fmt::Debug for AlpnProtocols {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_tuple("AlpnProtocols");
        for p in self.0.iter() {
            if let Ok(s) = std::str::from_utf8(p) {
                dbg.field(&s);
            } else {
                dbg.field(p);
            }
        }
        dbg.finish()
    }
}
