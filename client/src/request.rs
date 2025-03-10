use std::time::Duration;

use futures_core::Stream;
use tokio::time::Instant;
use xitca_http::{
    bytes::Bytes,
    error::BodyError,
    http::{
        self,
        header::{HeaderMap, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE},
        Method, Version,
    },
};

use crate::{
    body::{RequestBody, ResponseBody, StreamBody},
    client::Client,
    connect::Connect,
    connection::Connection,
    error::{Error, TimeoutError},
    response::DefaultResponse,
    timeout::Timeout,
    uri::Uri,
};

/// crate level HTTP request type.
pub struct Request<'a, B = StreamBody> {
    /// HTTP request type from [http] crate.
    req: http::Request<RequestBody<B>>,
    /// Referece to Client instance.
    client: &'a Client,
    /// Request level timeout setting. When Some(Duration) would override
    /// timeout configuration from Client.
    timeout: Duration,
}

impl<'a, B, E> Request<'a, B>
where
    B: Stream<Item = Result<Bytes, E>>,
    BodyError: From<E>,
{
    pub(crate) fn new(req: http::Request<RequestBody<B>>, client: &'a Client) -> Self {
        Self {
            req,
            client,
            timeout: client.timeout_config.request_timeout,
        }
    }

    /// Returns request's headers.
    #[inline]
    pub fn headers(&self) -> &HeaderMap {
        self.req.headers()
    }

    /// Returns request's mutable headers.
    #[inline]
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        self.req.headers_mut()
    }

    /// Set HTTP method of this request.
    #[inline]
    pub fn method(mut self, method: Method) -> Self {
        *self.req.method_mut() = method;
        self
    }

    #[doc(hidden)]
    /// Set HTTP version of this request.
    ///
    /// By default request's HTTP version depends on network stream
    pub fn version(mut self, version: Version) -> Self {
        *self.req.version_mut() = version;
        self
    }

    /// Set timeout of this request.
    ///
    /// The value passed would override global [TimeoutConfig].
    #[inline]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Use text(utf-8 encoded) as request body.
    ///
    /// [CONTENT_TYPE] header would be set with value: `text/plain; charset=utf-8`.
    pub fn text<B1>(mut self, text: B1) -> Request<'a, B>
    where
        Bytes: From<B1>,
    {
        self.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));

        self.body(text)
    }

    #[cfg(feature = "json")]
    /// Use json object as request body.
    pub fn json(mut self, body: impl serde::ser::Serialize) -> Result<Request<'a, B>, Error> {
        // TODO: handle serialize error.
        let body = serde_json::to_vec(&body).unwrap();

        self.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        Ok(self.body(body))
    }

    /// Use pre allocated bytes as request body.
    ///
    /// Input type must implement [From] trait with [Bytes].
    pub fn body<B1>(mut self, body: B1) -> Request<'a, B>
    where
        Bytes: From<B1>,
    {
        let bytes = Bytes::from(body);
        self.headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from(bytes.len()));
        self.map_body(move |_| RequestBody::bytes(bytes))
    }

    /// Use streaming type as request body.
    #[inline]
    pub fn stream<B1, E1>(self, body: B1) -> Request<'a, B1>
    where
        B1: Stream<Item = Result<Bytes, E1>>,
        BodyError: From<E1>,
    {
        self.map_body(move |_| RequestBody::stream(body))
    }

    fn map_body<F, B1, E1>(self, f: F) -> Request<'a, B1>
    where
        F: FnOnce(RequestBody<B>) -> RequestBody<B1>,
        B1: Stream<Item = Result<Bytes, E1>>,
        BodyError: From<E1>,
    {
        let Self { req, client, timeout } = self;
        let (parts, body_old) = req.into_parts();

        let body = f(body_old);
        let req = http::Request::from_parts(parts, body);

        Request::new(req, client).timeout(timeout)
    }

    /// Send the request and return response asynchronously.
    pub async fn send(self) -> Result<DefaultResponse<'a>, Error> {
        let Self { req, client, timeout } = self;

        let uri = Uri::try_parse(req.uri())?;

        // Try to grab a connection from pool.
        let mut conn = client.pool.acquire(&uri).await?;

        let conn_is_none = conn.is_none();

        // setup timer according to outcome and timeout configs.
        let dur = if conn_is_none {
            client.timeout_config.resolve_timeout
        } else {
            timeout
        };

        // heap allocate timer so it can be moved to Response type afterwards
        let mut timer = Box::pin(tokio::time::sleep(dur));

        // Nothing in the pool. construct new connection and add it to Conn.
        if conn_is_none {
            let mut connect = Connect::new(uri);
            let c = client.make_connection(&mut connect, &mut timer, req.version()).await?;
            conn.add(c);
        }

        let date = client.date_service.handle();

        timer
            .as_mut()
            .reset(Instant::now() + client.timeout_config.request_timeout);

        let res = match &mut *conn {
            Connection::Tcp(stream) => crate::h1::proto::send(stream, date, req).timeout(timer.as_mut()).await,
            Connection::Tls(stream) => crate::h1::proto::send(stream, date, req).timeout(timer.as_mut()).await,
            #[cfg(unix)]
            Connection::Unix(stream) => crate::h1::proto::send(stream, date, req).timeout(timer.as_mut()).await,
            #[cfg(feature = "http2")]
            Connection::H2(stream) => {
                return match crate::h2::proto::send(stream, date, req).timeout(timer.as_mut()).await {
                    Ok(Ok(res)) => {
                        let timeout = client.timeout_config.response_timeout;
                        Ok(DefaultResponse::new(res, timer, timeout))
                    }
                    Ok(Err(e)) => {
                        conn.destroy_on_drop();
                        Err(e.into())
                    }
                    Err(_) => {
                        conn.destroy_on_drop();
                        Err(TimeoutError::Request.into())
                    }
                }
            }
            #[cfg(feature = "http3")]
            Connection::H3(c) => {
                return match crate::h3::proto::send(c, date, req).timeout(timer.as_mut()).await {
                    Ok(Ok(res)) => {
                        let timeout = client.timeout_config.response_timeout;
                        Ok(DefaultResponse::new(res, timer, timeout))
                    }
                    Ok(Err(e)) => {
                        conn.destroy_on_drop();
                        Err(e.into())
                    }
                    Err(_) => {
                        conn.destroy_on_drop();
                        Err(TimeoutError::Request.into())
                    }
                }
            }
        };
        match res {
            Ok(Ok((res, buf, decoder, is_close))) => {
                if is_close {
                    conn.destroy_on_drop();
                }

                let body = crate::h1::body::ResponseBody::new(conn, buf, decoder);
                let res = res.map(|_| ResponseBody::H1(body));
                let timeout = client.timeout_config.response_timeout;

                Ok(DefaultResponse::new(res, timer, timeout))
            }
            Ok(Err(e)) => {
                conn.destroy_on_drop();
                Err(e.into())
            }
            Err(_) => {
                conn.destroy_on_drop();
                Err(TimeoutError::Request.into())
            }
        }
    }
}
