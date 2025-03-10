use std::{fmt, future::Future};

use futures_core::Stream;
use http::{Request, Response};
use tokio::pin;
use xitca_io::io::{AsyncRead, AsyncWrite};
use xitca_service::Service;

use crate::{
    body::ResponseBody,
    bytes::Bytes,
    error::{BodyError, HttpServiceError, TimeoutError},
    service::HttpService,
    util::futures::Timeout,
};

use super::{body::RequestBody, proto::Dispatcher};

pub type H2Service<S, A, const HEADER_LIMIT: usize, const READ_BUF_LIMIT: usize, const WRITE_BUF_LIMIT: usize> =
    HttpService<S, RequestBody, (), A, HEADER_LIMIT, READ_BUF_LIMIT, WRITE_BUF_LIMIT>;

impl<St, S, B, E, A, TlsSt, const HEADER_LIMIT: usize, const READ_BUF_LIMIT: usize, const WRITE_BUF_LIMIT: usize>
    Service<St> for H2Service<S, A, HEADER_LIMIT, READ_BUF_LIMIT, WRITE_BUF_LIMIT>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody<B>>> + 'static,
    S::Error: fmt::Debug,

    A: Service<St, Response = TlsSt> + 'static,

    B: Stream<Item = Result<Bytes, E>> + 'static,
    E: 'static,
    BodyError: From<E>,

    St: AsyncRead + AsyncWrite + Unpin,
    TlsSt: AsyncRead + AsyncWrite + Unpin,

    HttpServiceError<S::Error>: From<A::Error>,
{
    type Response = ();
    type Error = HttpServiceError<S::Error>;
    type Ready<'f> = impl Future<Output = Result<(), Self::Error>>;
    type Future<'f> = impl Future<Output = Result<Self::Response, Self::Error>>;

    #[inline]
    fn ready(&self) -> Self::Ready<'_> {
        async move {
            self.tls_acceptor
                .ready()
                .await
                .map_err(|_| HttpServiceError::ServiceReady)?;

            self.service.ready().await.map_err(|_| HttpServiceError::ServiceReady)
        }
    }

    fn call(&self, io: St) -> Self::Future<'_> {
        async move {
            // tls accept timer.
            let timer = self.keep_alive();
            pin!(timer);

            let tls_stream = self
                .tls_acceptor
                .call(io)
                .timeout(timer.as_mut())
                .await
                .map_err(|_| HttpServiceError::Timeout(TimeoutError::TlsAccept))??;

            // update timer to first request timeout.
            self.update_first_request_deadline(timer.as_mut());

            let mut conn = ::h2::server::handshake(tls_stream)
                .timeout(timer.as_mut())
                .await
                .map_err(|_| HttpServiceError::Timeout(TimeoutError::H2Handshake))??;

            let dispatcher = Dispatcher::new(
                &mut conn,
                timer.as_mut(),
                self.config.keep_alive_timeout,
                &self.service,
                self.date.get(),
            );

            dispatcher.run().await?;

            Ok(())
        }
    }
}
