use crate::{proto, LookupAddr, Profile, Receiver};
use futures::prelude::*;
use http_body::Body;
use linkerd2_proxy_api::destination::{self as api, destination_client::DestinationClient};
use linkerd_cache::{self as cache, Cache};
use linkerd_error::{Infallible, Recover};
use linkerd_stack::{FutureService, NewService, Oneshot, Service, ServiceExt};
use linkerd_tonic_watch::StreamWatch;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tonic::{body::BoxBody, client::GrpcService};
use tracing::debug;

/// Creates watches on service profiles.
#[derive(Clone, Debug)]
pub struct Client<R, S>
where
    R: Recover<tonic::Status> + Clone + Send + 'static,
    R::Backoff: Unpin + Send,
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
{
    inner: Cache<LookupAddr, NewWatch<R, S>>,
}

#[pin_project::pin_project]
pub struct ResponseFuture {
    #[pin]
    inner: Oneshot<WatchService, ()>,
    handle: Option<cache::Handle>,
}

#[derive(Clone, Debug)]
struct NewWatch<R, S> {
    capacity: usize,
    inner: StreamWatch<R, Inner<S>>,
}

#[derive(Clone, Debug)]
struct CloneReceiver(tokio::sync::watch::Receiver<Profile>);

/// Wraps the destination service to hide protobuf types.
#[derive(Clone, Debug)]
struct Inner<S> {
    client: DestinationClient<S>,
    context_token: Arc<str>,
}

// === impl Client ===

impl<R, S> Client<R, S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
    R: Recover<tonic::Status> + Send + Clone + 'static,
    R::Backoff: Unpin + Send,
{
    pub fn new(
        recover: R,
        inner: S,
        context_token: impl Into<Arc<str>>,
        idle: tokio::time::Duration,
        capacity: usize,
    ) -> Self {
        let inner = StreamWatch::new(recover, Inner::new(context_token.into(), inner));
        Self {
            inner: Cache::new(idle, NewWatch { capacity, inner }),
        }
    }
}

impl<R, S> Service<LookupAddr> for Client<R, S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
    R: Recover<tonic::Status> + Send + Clone + 'static,
    R::Backoff: Unpin + Send,
{
    type Response = Option<Receiver>;
    type Error = Infallible;
    type Future = ResponseFuture;

    #[inline]
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, addr: LookupAddr) -> Self::Future {
        // Obtain a cached service for the lookup address.
        let (svc, handle) = self.inner.new_service(addr).into_parts();
        ResponseFuture {
            inner: svc.oneshot(()),
            handle: Some(handle),
        }
    }
}

// === impl ResponseFuture ===

impl Future for ResponseFuture {
    type Output = Result<Option<Receiver>, Infallible>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match futures::ready!(this.inner.poll(cx)) {
            Ok(rx) => Poll::Ready(Ok(Some(Receiver::new(rx, this.handle.take().unwrap())))),
            Err(status) => {
                // Retries are applied internally, so if this failed we hit an unrecoverable
                // response; return no receiver. In this case we don't retain the cache handle,
                // so the lookup should idle out soon after.
                debug!(%status, "Ignoring profile");
                Poll::Ready(Ok(None))
            }
        }
    }
}

// === impl NewWatch ===

type WatchService = tower::buffer::Buffer<
    FutureService<
        Pin<Box<dyn Future<Output = Result<CloneReceiver, tonic::Status>> + Send + 'static>>,
        CloneReceiver,
    >,
    (),
>;

impl<R, S> NewService<LookupAddr> for NewWatch<R, S>
where
    R: Recover<tonic::Status> + Clone + Send + 'static,
    R::Backoff: Unpin + Send,
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
{
    type Service = WatchService;

    fn new_service(&self, target: LookupAddr) -> Self::Service {
        // Initiate a watch on the profile.
        let watch = self
            .inner
            .clone()
            .spawn_watch(target)
            .map_ok(|rsp| CloneReceiver(rsp.into_inner()));

        // Return a unit-service immediately; but buffer requests to clone the watch until the
        // lookup succeeds.
        tower::buffer::Buffer::new(FutureService::new(Box::pin(watch)), self.capacity)
    }
}

// === impl CloneReceiver ===

impl Service<()> for CloneReceiver {
    type Response = tokio::sync::watch::Receiver<Profile>;
    type Error = tonic::Status;
    type Future = future::Ready<Result<Self::Response, Self::Error>>;

    #[inline]
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, _: ()) -> Self::Future {
        future::ok(self.0.clone())
    }
}

// === impl Inner ===

type InnerStream = futures::stream::BoxStream<'static, Result<Profile, tonic::Status>>;

type InnerFuture =
    futures::future::BoxFuture<'static, Result<tonic::Response<InnerStream>, tonic::Status>>;

impl<S> Inner<S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
{
    fn new(context_token: Arc<str>, inner: S) -> Self {
        Self {
            context_token,
            client: DestinationClient::new(inner),
        }
    }
}

impl<S> Service<LookupAddr> for Inner<S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
{
    type Response = tonic::Response<InnerStream>;
    type Error = tonic::Status;
    type Future = InnerFuture;

    #[inline]
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Tonic clients do not expose readiness.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, LookupAddr(addr): LookupAddr) -> Self::Future {
        let req = api::GetDestination {
            path: addr.to_string(),
            context_token: self.context_token.to_string(),
            ..Default::default()
        };

        let mut client = self.client.clone();
        Box::pin(async move {
            let rsp = client.get_profile(req).await?;
            Ok(rsp.map(|s| {
                Box::pin(s.map_ok(move |p| proto::convert_profile(p, addr.port()))) as InnerStream
            }))
        })
    }
}
