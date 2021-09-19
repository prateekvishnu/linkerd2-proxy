use super::{GetProfile, LookupAddr, Receiver};
use futures::prelude::*;
use linkerd_error::{Error, Result};
use linkerd_stack::{layer, Either, ExtractParam, FutureService, NewService};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Clone, Debug)]
pub struct NewDiscover<P, G, N> {
    params: P,
    get_profile: G,
    inner: N,
}

impl<P, G, N> NewDiscover<P, G, N> {
    pub fn layer(get_profile: G, params: P) -> impl layer::Layer<N, Service = Self> + Clone
    where
        P: Clone,
        G: GetProfile + Clone,
    {
        layer::mk(move |inner| Self {
            params: params.clone(),
            get_profile: get_profile.clone(),
            inner,
        })
    }
}

impl<T, P, G, N> NewService<T> for NewDiscover<P, G, N>
where
    T: Send + 'static,
    P: ExtractParam<Option<LookupAddr>, T>,
    G: GetProfile + Send + Clone + 'static,
    G::Future: Send + 'static,
    G::Error: Into<Error> + Send,
    N: NewService<(Option<Receiver>, T)> + Clone + Send + 'static,
{
    type Service = Either<FutureService<MakeFuture<T, G::Future, N>, N::Service>, N::Service>;

    fn new_service(&self, target: T) -> Self::Service {
        match self.params.extract_param(&target) {
            Some(l) => {
                let get_profile = self.get_profile.get_profile(l);
                Either::A(FutureService::new(MakeFuture {
                    target: Some(target),
                    new_service: self.inner.clone(),
                    get_profile,
                }))
            }
            None => Either::B(self.inner.new_service((None, target))),
        }
    }
}

#[pin_project::pin_project]
pub struct MakeFuture<T, F, N> {
    #[pin]
    get_profile: F,
    target: Option<T>,
    new_service: N,
}

impl<T, F, N> Future for MakeFuture<T, F, N>
where
    F: TryFuture + 'static,
    F::Error: Into<Error>,
    N: NewService<(F::Ok, T)>,
{
    type Output = Result<N::Service>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let rx = futures::ready!(this.get_profile.try_poll(cx)).map_err(Into::into)?;
        let target = this.target.take().unwrap();
        let svc = this.new_service.new_service((rx, target));
        Poll::Ready(Ok(svc))
    }
}
