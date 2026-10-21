//! The HTTP/1.1 server loop (ADR 0008).
//!
//! `axum::serve` gives hyper no timer. Hyper's header read timeout never
//! starts without one (ADR 0008 §Context). This loop gives hyper a timer.
//! It serves each connection with `hyper::server::conn::http1::Builder`.
//! It does not use `hyper_util::server::conn::auto`.
//!
//! The auto builder still reads the HTTP/2 preface when a caller sets
//! `http1_only()` and calls `serve_connection_with_upgrades` (amendment of
//! 2026-10-21). It can then answer a client with HTTP/2. The plain HTTP/1
//! builder never reads that preface. The relay then serves HTTP/1.1 only.
//! The header read timeout covers every connection.

use std::{future::Future, net::SocketAddr, pin::pin, time::Duration};

use axum::{Router, body::Body, extract::ConnectInfo, http::Request};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use tokio::{net::TcpListener, net::TcpStream, sync::watch};
use tower::ServiceExt as _;

/// Runs the HTTP/1.1 server loop (ADR 0008).
///
/// The loop accepts each connection and gives it the TCP peer address, in
/// `axum::extract::ConnectInfo<SocketAddr>`, on every request (ADR 0007
/// §The Client Address). It serves the connection with
/// `header_read_timeout` as the hyper header read timeout. A connection
/// with no full request head, and an idle keep-alive connection, close
/// after that time (ADR 0008 §Two Timeouts). A WebSocket upgrade stays
/// open past the timeout (ADR 0008 §What The Timeouts Do Not Stop).
///
/// An accept error does not stop the loop. The loop logs the error, waits
/// one second, and tries again.
///
/// When `shutdown` completes, the loop stops accepting new connections.
/// It tells each open connection to start a graceful shutdown, then waits
/// for every connection to end. This wait has no timeout, the same as
/// `axum::serve` before this change.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    header_read_timeout: Duration,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);

    // `signal_tx` tells each connection to start a graceful shutdown. Each
    // connection task holds a clone of it. `Sender::closed()` on a clone
    // resolves only after the one receiver, `signal_rx`, is dropped. This
    // is the pattern of `handle_connection` in axum 0.8.9's `axum::serve`.
    let (signal_tx, signal_rx) = watch::channel(());
    // `close_tx` lets this loop wait until every connection task has
    // ended. Each connection task holds a clone of `close_rx`. It drops
    // that clone when the task ends.
    let (close_tx, close_rx) = watch::channel(());

    let mut shutdown = pin!(shutdown);

    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                // A client that closes or resets its connection before the
                // accept affects only that connection. Accept the next one
                // with no pause, as `axum::serve` does.
                Err(error) if is_connection_error(&error) => continue,
                Err(error) => {
                    tracing::warn!(%error, "accept error; retrying in 1 second");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = shutdown.as_mut() => break,
        };

        let router = router.clone();
        let builder = builder.clone();
        let signal_tx = signal_tx.clone();
        let close_rx = close_rx.clone();

        tokio::spawn(async move {
            serve_connection(stream, peer, router, builder, signal_tx).await;
            drop(close_rx);
        });
    }

    // The shutdown signal arrived. Dropping `signal_rx` tells every open
    // connection to start a graceful shutdown.
    drop(signal_rx);
    // Drop this function's own `close_rx` too. `close_tx.closed()` then
    // resolves once every connection task also drops its clone, with no
    // timeout on that wait.
    drop(close_rx);
    tracing::info!("waiting for open connections to end");
    close_tx.closed().await;

    Ok(())
}

/// Serves one accepted connection until it ends, or until `signal_tx`
/// signals a graceful shutdown.
///
/// Inserts `ConnectInfo(peer)` into the extensions of each request on
/// this connection (ADR 0007 §The Client Address). A WebSocket upgrade
/// hands the connection to the upgraded stream. This function's own
/// future then ends, and the upgraded stream stays open on its own.
async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    router: Router,
    builder: http1::Builder,
    signal_tx: watch::Sender<()>,
) {
    let service = router.map_request(move |mut request: Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        request.map(Body::new)
    });
    let service = TowerToHyperService::new(service);

    let connection = builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    let mut connection = pin!(connection);

    tokio::select! {
        result = connection.as_mut() => {
            // A connection that the header read timeout closes ends with an
            // error. A client can cause many of these, so log them at
            // `debug` only, as `axum::serve` logs them at `trace`.
            if let Err(error) = result {
                tracing::debug!(%error, %peer, "connection error");
            }
        }
        () = signal_tx.closed() => {
            connection.as_mut().graceful_shutdown();
            if let Err(error) = connection.as_mut().await {
                tracing::debug!(%error, %peer, "connection error during graceful shutdown");
            }
        }
    }
}

/// Returns `true` for an accept error that belongs to one connection only:
/// the client refused, closed or reset it. This is the list of
/// `is_connection_error` in axum 0.8.9.
fn is_connection_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}
