#![deny(warnings, rust_2018_idioms)]
#![forbid(unsafe_code)]

pub mod client;
pub mod server;

pub use self::{
    client::{Client, ConditionalClientTls, NoClientTls, ServerId},
    server::{ClientId, ConditionalServerTls, NewDetectTls, NoServerTls, ServerTls},
};
use linkerd_error::Result;
pub use linkerd_identity::LocalId;

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct NegotiatedProtocol(pub Vec<u8>);

/// Indicates a negotiated protocol.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct NegotiatedProtocolRef<'t>(pub &'t [u8]);

impl NegotiatedProtocol {
    pub fn as_ref(&self) -> NegotiatedProtocolRef<'_> {
        NegotiatedProtocolRef(&self.0)
    }
}

impl std::fmt::Debug for NegotiatedProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        NegotiatedProtocolRef(&self.0).fmt(f)
    }
}

impl NegotiatedProtocolRef<'_> {
    pub fn to_owned(self) -> NegotiatedProtocol {
        NegotiatedProtocol(self.0.into())
    }
}

impl From<NegotiatedProtocolRef<'_>> for NegotiatedProtocol {
    fn from(protocol: NegotiatedProtocolRef<'_>) -> NegotiatedProtocol {
        protocol.to_owned()
    }
}

#[async_trait::async_trait]
pub trait Server {}

impl std::fmt::Debug for NegotiatedProtocolRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match std::str::from_utf8(self.0) {
            Ok(s) => s.fmt(f),
            Err(_) => self.0.fmt(f),
        }
    }
}

#[async_trait::async_trait]
pub trait CertificateAuthority<C>: std::convert::TryFrom<C> {
    type Credentials: Credentials;

    async fn spawn_credentials(self) -> Result<Self::Credentials>;
}

pub trait Credentials {
    type Client;
    type Server;

    fn id(&self) -> LocalId;

    fn expiry(&self) -> std::time::SystemTime;

    fn client(&self) -> Self::Client;

    fn server(&self) -> Self::Server;
}
