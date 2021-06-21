use anyhow::{Error, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use structopt::StructOpt;

#[derive(Clone, Debug, StructOpt)]
pub struct Args {
    #[structopt(long, default_value = "0.0.0.0:9080")]
    admin_addr: SocketAddr,
    // #[structopt(long, default_value = "0.0.0.0:8070")]
    // grpc_addr: SocketAddr,
    #[structopt(long, default_value = "0.0.0.0:8080")]
    http_addr: SocketAddr,
    // #[structopt(long, default_value = "0.0.0.0:8090")]
    // tcp_addr: SocketAddr,
}

impl Args {
    pub async fn run(self) -> Result<()> {
        tokio::select! {
            res = serve_admin(self.admin_addr) => res,
            res = serve_http(self.http_addr) => res,
        }
    }
}

async fn serve_admin(addr: SocketAddr) -> Result<()> {
    hyper::Server::bind(&addr)
        .serve(hyper::service::make_service_fn(|_| async {
            //|req: hyper::Request<hyper::Body>| async move { handle_http(req).await },
            Ok::<_, Error>(hyper::service::service_fn(handle_admin))
        }))
        .await?;

    Ok(())
}

async fn handle_admin(req: hyper::Request<hyper::Body>) -> Result<hyper::Response<hyper::Body>> {
    match (req.method(), req.uri().path()) {
        (&hyper::Method::GET | &hyper::Method::HEAD, "/live" | "/ready") => {
            Ok(hyper::Response::builder()
                .status(hyper::StatusCode::OK)
                .body(hyper::Body::default())
                .unwrap())
        }

        _ => Ok(hyper::Response::builder()
            .status(hyper::StatusCode::NOT_FOUND)
            .body(hyper::Body::default())
            .unwrap()),
    }
}

async fn serve_http(addr: SocketAddr) -> Result<()> {
    hyper::Server::bind(&addr)
        .serve(hyper::service::make_service_fn(|_| async {
            Ok::<_, Error>(hyper::service::service_fn(handle_http))
        }))
        .await?;

    Ok(())
}

#[derive(Deserialize, Serialize)]
struct RequestSummary {
    pub version: String,
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
}

async fn handle_http(req: hyper::Request<hyper::Body>) -> Result<hyper::Response<hyper::Body>> {
    match req.uri().path() {
        "/request-summary" => {
            let summary = RequestSummary {
                version: format!("{:?}", req.version()),
                method: req.method().to_string(),
                uri: req.uri().to_string(),
                headers: req
                    .headers()
                    .iter()
                    .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
                    .collect(),
            };
            Ok(hyper::Response::builder()
                .status(hyper::StatusCode::OK)
                .body(hyper::Body::from(
                    serde_json::to_string_pretty(&summary).unwrap(),
                ))
                .unwrap())
        }
        _ => Ok(hyper::Response::builder()
            .status(hyper::StatusCode::NOT_FOUND)
            .body(hyper::Body::default())
            .unwrap()),
    }
}
