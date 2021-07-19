use linkerd_app_core::{svc, transport::OrigDstAddr, Error as BoxError};
use std::{collections::HashMap, sync::Arc, time::Duration};
use thiserror::Error;

type Svc<I> = svc::BoxService<I, (), BoxError>;
type NewSvc<T, I> = svc::BoxNewService<T, Svc<I>>;

#[derive(Clone, Debug)]
pub struct NewAcceptPorts<T, I> {
    ports: Arc<HashMap<u16, PortPolicy>>,
    default: PortPolicy,
    detect: NewSvc<T, I>,
    opaque: NewSvc<T, I>,
}

#[derive(Copy, Clone, Debug)]
pub enum PortPolicy {
    Detect { timeout: Duration },
    Opaque,
    Reject,
}

#[derive(Clone, Debug, Error)]
#[error("connection rejected by policy")]
pub struct Rejected(());

impl<T, I> NewAcceptPorts<T, I> {
    pub fn new(
        ports: impl IntoIterator<Item = (u16, PortPolicy)>,
        default: PortPolicy,
        detect: NewSvc<T, I>,
        opaque: NewSvc<T, I>,
    ) -> Self {
        let ports = Arc::new(ports.into_iter().collect());

        Self {
            ports,
            default,
            detect,
            opaque,
        }
    }
}

impl<T, I> svc::NewService<T> for NewAcceptPorts<T, I>
where
    T: svc::Param<OrigDstAddr>,
    I: 'static,
{
    type Service = svc::BoxService<I, (), BoxError>;

    fn new_service(&mut self, target: T) -> Self::Service {
        let OrigDstAddr(a) = target.param();

        match self.ports.get(&a.port()).unwrap_or(&self.default) {
            PortPolicy::Detect { timeout: _ } => {
                todo!("Detect TLS and HTTP");
            }
            PortPolicy::Opaque => self.opaque.new_service(target),
            PortPolicy::Reject => {
                let reject = svc::stack::ResultService::<Svc<I>>::err(Rejected(()));
                svc::BoxService::new(reject)
            }
        }
    }
}
