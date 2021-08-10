mod router;
mod server;
mod set_identity_header;
#[cfg(test)]
mod tests;

fn trace_labels() -> std::collections::HashMap<String, String> {
    let mut l = std::collections::HashMap::new();
    l.insert("direction".to_string(), "inbound".to_string());
    l
}

/// Test-support helpers used by both the stack tests and the fuzz logic.
#[cfg(any(test, fuzzing))]
pub(crate) mod test_util {
    pub use crate::test_util::{
        support::{connect::Connect, http_util, profile, resolver},
        *,
    };
    use crate::{Config, Inbound};
    use hyper::{Body, Request, Response};
    use linkerd_app_core::{
        identity, io,
        proxy::http,
        svc::{self, Param},
        tls,
        transport::{ClientAddr, OrigDstAddr, Remote, ServerAddr},
        ProxyRuntime,
    };
    use support::*;

    #[derive(Clone, Debug)]
    pub(crate) struct Target(http::Version);

    #[tracing::instrument]
    pub(crate) fn hello_server(
        http: hyper::server::conn::Http,
    ) -> impl Fn(Remote<ServerAddr>) -> io::Result<io::BoxedIo> {
        move |endpoint| {
            let span = tracing::info_span!("hello_server", ?endpoint);
            let _e = span.enter();
            tracing::info!("mock connecting");
            let (client_io, server_io) = support::io::duplex(4096);
            let hello_svc = hyper::service::service_fn(|request: Request<Body>| async move {
                tracing::info!(?request);
                Ok::<_, io::Error>(Response::new(Body::from("Hello world!")))
            });
            tokio::spawn(
                http.serve_connection(server_io, hello_svc)
                    .in_current_span(),
            );
            Ok(io::BoxedIo::new(client_io))
        }
    }

    pub(crate) fn build_server<I>(
        cfg: Config,
        rt: ProxyRuntime,
        profiles: resolver::Profiles,
        connect: Connect<Remote<ServerAddr>>,
    ) -> svc::BoxNewTcp<Target, I>
    where
        I: io::AsyncRead + io::AsyncWrite + io::PeerAddr + Send + Unpin + 'static,
    {
        Inbound::new(cfg, rt)
            .with_stack(connect)
            .map_stack(|cfg, _, s| {
                s.push_map_target(|p| Remote(ServerAddr(([127, 0, 0, 1], p).into())))
                    .push_map_target(|t| Param::<u16>::param(&t))
                    .push_connect_timeout(cfg.proxy.connect.timeout)
            })
            .push_http_router(profiles)
            .push_http_server()
            .into_inner()
    }

    // === impl Target ===

    impl Target {
        pub(crate) const HTTP1: Self = Self(http::Version::Http1);
        #[cfg(test)] // fuzzer doesn't currently use h2
        pub(crate) const H2: Self = Self(http::Version::H2);

        pub(crate) fn addr() -> SocketAddr {
            ([127, 0, 0, 1], 80).into()
        }
    }

    impl svc::Param<OrigDstAddr> for Target {
        fn param(&self) -> OrigDstAddr {
            OrigDstAddr(([192, 0, 2, 2], 80).into())
        }
    }

    impl svc::Param<Remote<ServerAddr>> for Target {
        fn param(&self) -> Remote<ServerAddr> {
            Remote(ServerAddr(Self::addr()))
        }
    }

    impl svc::Param<Remote<ClientAddr>> for Target {
        fn param(&self) -> Remote<ClientAddr> {
            Remote(ClientAddr(([192, 0, 2, 3], 50000).into()))
        }
    }

    impl svc::Param<http::Version> for Target {
        fn param(&self) -> http::Version {
            self.0
        }
    }

    impl svc::Param<tls::ConditionalServerTls> for Target {
        fn param(&self) -> tls::ConditionalServerTls {
            tls::ConditionalServerTls::None(tls::NoServerTls::NoClientHello)
        }
    }

    impl svc::Param<http::normalize_uri::DefaultAuthority> for Target {
        fn param(&self) -> http::normalize_uri::DefaultAuthority {
            http::normalize_uri::DefaultAuthority(None)
        }
    }

    impl svc::Param<Option<identity::Name>> for Target {
        fn param(&self) -> Option<identity::Name> {
            None
        }
    }
}

#[cfg(fuzzing)]
pub mod fuzz {
    use super::test_util::{
        build_server, default_config, hello_server, http_util, runtime,
        support::{self, profile},
        Target,
    };
    use hyper::{client::conn::Builder as ClientBuilder, Body, Request};
    use libfuzzer_sys::arbitrary::Arbitrary;
    use linkerd_app_core::{svc::NewService, NameAddr};

    use std::{fmt, str};

    #[derive(Arbitrary)]
    pub struct HttpRequestSpec {
        pub uri: Vec<u8>,
        pub header_name: Vec<u8>,
        pub header_value: Vec<u8>,
        pub http_method: bool,
    }

    pub async fn fuzz_entry_raw(requests: Vec<HttpRequestSpec>) {
        let mut server = hyper::server::conn::Http::new();
        server.http1_only(true);
        let mut client = ClientBuilder::new();
        let connect = support::connect().endpoint_fn_boxed(Target::addr(), hello_server(server));
        let profiles = profile::resolver();
        let profile_tx = profiles
            .profile_tx(NameAddr::from_str_and_port("foo.svc.cluster.local", 5550).unwrap());
        profile_tx.send(profile::Profile::default()).unwrap();

        // Build the outbound server
        let cfg = default_config();
        let (rt, _shutdown) = runtime();
        let server = build_server(cfg, rt, profiles, connect).new_service(Target::HTTP1);
        let (mut client, bg) = http_util::connect_and_accept(&mut client, server).await;

        // Now send all of the requests
        for inp in requests.iter() {
            if let Ok(uri) = std::str::from_utf8(&inp.uri[..]) {
                if let Ok(header_name) = std::str::from_utf8(&inp.header_name[..]) {
                    if let Ok(header_value) = std::str::from_utf8(&inp.header_value[..]) {
                        let http_method = if inp.http_method {
                            hyper::http::Method::GET
                        } else {
                            hyper::http::Method::POST
                        };

                        if let Ok(req) = Request::builder()
                            .method(http_method)
                            .uri(uri)
                            .header(header_name, header_value)
                            .body(Body::default())
                        {
                            let rsp = http_util::http_request(&mut client, req).await;
                            tracing::info!(?rsp);
                            if let Ok(rsp) = rsp {
                                let body = http_util::body_to_string(rsp.into_body()).await;
                                tracing::info!(?body);
                            }
                        }
                    }
                }
            }
        }

        drop(client);
        // It's okay if the background task returns an error, as this would
        // indicate that the proxy closed the connection --- which it will do on
        // invalid inputs. We want to ensure that the proxy doesn't crash in the
        // face of these inputs, and the background task will panic in this
        // case.
        let res = bg.await;
        tracing::info!(?res, "background tasks completed")
    }

    impl fmt::Debug for HttpRequestSpec {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // Custom `Debug` impl that formats the URI, header name, and header
            // value as strings if they are UTF-8, or falls back to raw bytes
            // otherwise.
            let mut dbg = f.debug_struct("HttpRequestSpec");
            dbg.field("http_method", &self.http_method);

            if let Ok(uri) = str::from_utf8(&self.uri[..]) {
                dbg.field("uri", &uri);
            } else {
                dbg.field("uri", &self.uri);
            }

            if let Ok(name) = str::from_utf8(&self.header_name[..]) {
                dbg.field("header_name", &name);
            } else {
                dbg.field("header_name", &self.header_name);
            }

            if let Ok(value) = str::from_utf8(&self.header_value[..]) {
                dbg.field("header_value", &value);
            } else {
                dbg.field("header_value", &self.header_value);
            }

            dbg.finish()
        }
    }
}
