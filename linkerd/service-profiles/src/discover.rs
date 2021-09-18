use super::{default::RecoverDefault, GetProfile, GetProfileService, LookupAddr, Receiver};
use futures::prelude::*;
use linkerd_stack::{layer, Either, ExtractParam, FutureService, NewService};
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug)]
pub struct NewDiscover<P, G, N> {
    params: P,
    get_profile: G,
    inner: N,
}

type DiscoverService<S, E> =
    FutureService<Pin<Box<dyn Future<Output = Result<S, E>> + Send + 'static>>, S>;

impl<P, G, N> NewDiscover<P, RecoverDefault<GetProfileService<G>>, N> {
    pub fn layer(get_profile: G, params: P) -> impl layer::Layer<N, Service = Self> + Clone
    where
        P: Clone,
        G: GetProfile + Clone,
    {
        let get_profile = RecoverDefault::new(get_profile.into_service());

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
    G: GetProfile + Clone,
    G::Future: Send + 'static,
    G::Error: Send,
    N: NewService<(Option<Receiver>, T)> + Clone + Send + 'static,
{
    type Service = Either<DiscoverService<N::Service, G::Error>, N::Service>;

    fn new_service(&self, target: T) -> Self::Service {
        let lookup = match self.params.extract_param(&target) {
            Some(lookup) => lookup,
            None => return Either::B(self.inner.new_service((None, target))),
        };

        let inner = self.inner.clone();
        Either::A(FutureService::new(Box::pin(
            self.get_profile
                .clone()
                .get_profile(lookup)
                .map_ok(move |rx| inner.new_service((rx, target))),
        )))
    }
}
