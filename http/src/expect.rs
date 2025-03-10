//! Default expect handler. Pass through request unconditionally.

use std::{
    future::{ready, Future, Ready},
    marker::PhantomData,
};

use xitca_service::{Service, ServiceFactory};

pub struct ExpectHandler<F>(PhantomData<F>);

impl<F> Default for ExpectHandler<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> ExpectHandler<F> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<F, Req> ServiceFactory<Req> for ExpectHandler<F>
where
    F: ServiceFactory<Req>,
{
    type Response = Req;
    type Error = F::Error;
    type Config = ();
    type Service = Self;
    type InitError = F::InitError;
    type Future = impl Future<Output = Result<Self::Service, Self::InitError>>;

    fn new_service(&self, _: Self::Config) -> Self::Future {
        async { Ok(Self::new()) }
    }
}

impl<F, Req> Service<Req> for ExpectHandler<F>
where
    F: ServiceFactory<Req>,
{
    type Response = Req;
    type Error = F::Error;
    type Ready<'f>
    where
        Self: 'f,
    = Ready<Result<(), Self::Error>>;
    type Future<'f>
    where
        Self: 'f,
    = impl Future<Output = Result<Self::Response, Self::Error>>;

    #[inline]
    fn ready(&self) -> Self::Ready<'_> {
        ready(Ok(()))
    }

    fn call(&self, req: Req) -> Self::Future<'_> {
        async move { Ok(req) }
    }
}
