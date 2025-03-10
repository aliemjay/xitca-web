use std::{fmt, future::Future};

use futures_core::Stream;
use http::{Request, Response};
use xitca_io::net::UdpStream;
use xitca_service::Service;

use crate::{
    body::ResponseBody,
    bytes::Bytes,
    error::{BodyError, HttpServiceError},
};

use super::{body::RequestBody, proto::Dispatcher};

pub struct H3Service<S> {
    service: S,
}

impl<S> H3Service<S> {
    /// Construct new Http3Service.
    /// No upgrade/expect services allowed in Http/3.
    pub fn new(service: S) -> Self {
        Self { service }
    }
}

impl<S, B, E> Service<UdpStream> for H3Service<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody<B>>> + 'static,
    S::Error: fmt::Debug,

    B: Stream<Item = Result<Bytes, E>> + 'static,
    E: 'static,
    BodyError: From<E>,
{
    type Response = ();
    type Error = HttpServiceError<S::Error>;
    type Ready<'f> = impl Future<Output = Result<(), Self::Error>>;
    type Future<'f> = impl Future<Output = Result<Self::Response, Self::Error>>;

    #[inline]
    fn ready(&self) -> Self::Ready<'_> {
        async move { self.service.ready().await.map_err(|_| HttpServiceError::ServiceReady) }
    }

    fn call(&self, stream: UdpStream) -> Self::Future<'_> {
        async move {
            let dispatcher = Dispatcher::new(stream, &self.service);

            dispatcher.run().await?;

            Ok(())
        }
    }
}
