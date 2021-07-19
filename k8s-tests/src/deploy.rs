#![allow(dead_code)] // FIXME

use crate::FIELD_MANAGER;
use anyhow::Result;
use k8s_openapi::api::{apps::v1 as apps, batch::v1 as batch, core::v1 as core};
use kube::ResourceExt;
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    str::FromStr,
    time,
};
use structopt::StructOpt;
use tracing::{debug, info};

#[derive(Clone, Debug, StructOpt)]
pub struct Args {
    #[structopt(long, default_value = "ghcr.io/linkerd/k8s-tests:latest")]
    image: String,

    #[structopt(long)]
    proxy_image: String,

    #[structopt(long)]
    dry_run: bool,

    #[structopt(default_value = "default")]
    id: String,
}

#[derive(Clone, Debug)]
pub enum LinkerdConfig {
    Enabled { config: HashMap<String, String> },
    Ingress { config: HashMap<String, String> },
}

pub struct TestNs {
    pub id: String,
    pub image: Image,
    pub proxy_image: Image,
}

#[derive(Clone, Debug)]
pub struct Image {
    pub repo: String,
    pub tag: Option<String>,
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.repo.fmt(f)?;
        if let Some(t) = self.tag.as_deref() {
            write!(f, ":{}", t)?;
        }
        Ok(())
    }
}

impl Args {
    pub async fn run(self, client: kube::Client) -> Result<()> {
        let dry_run = self.dry_run;
        let tns = TestNs {
            id: self.id,
            image: self.image.parse()?,
            proxy_image: self.proxy_image.parse()?,
        };

        super::create_crds(client.clone(), time::Duration::from_secs(10), dry_run).await?;

        let server = super::create_deploy(
            client.clone(),
            tns.server("uninjected", None),
            time::Duration::from_secs(60),
            dry_run,
        )
        .await?;
        info!(deploy = %server.name(), ?dry_run, "Created");
        debug!(server = format_args!("{:#?}", server));

        let service = {
            let params = kube::api::PostParams {
                dry_run,
                field_manager: Some(FIELD_MANAGER.to_string()),
            };
            kube::Api::namespaced(client, &*tns.ns())
                .create(&params, &tns.service("uninjected"))
                .await?
        };
        info!(service =  %service.name(), ?dry_run, "Created");
        debug!(service = format_args!("{:#?}", service));

        debug!(job = format_args!("{:#?}", tns.runner("uninjected", None)));

        Ok(())
    }
}

impl FromStr for Image {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (repo, tag) = match s.rsplit_once(':') {
            None => (s.to_string(), None),
            Some((r, t)) => (r.to_string(), Some(t.to_string())),
        };
        Ok(Self { repo, tag })
    }
}

impl TestNs {
    fn ns(&self) -> String {
        format!("linkerd-k8s-test-{}", self.id)
    }

    pub fn namespace(&self) -> core::Namespace {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": self.ns(),
            },
        }))
        .expect("must parse ns")
    }

    pub fn runner(&self, name: &str, linkerd: Option<&LinkerdConfig>) -> batch::Job {
        let labels = Some(("runner".to_string(), name.to_string()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let pod_annotations = linkerd
            .map(|c| c.to_annotations(&self.proxy_image))
            .unwrap_or_default();

        serde_json::from_value(serde_json::json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": {
                "namespace": format!("linkerd-k8s-test-{}", self.id),
                "name": format!("runner-{}", name),
                "labels": labels,
            },
            "spec": {
                "template": {
                    "metadata": {
                        "labels": labels,
                        "annotations": pod_annotations,
                    },
                    "spec": {
                        "containers": [
                            {
                                "name": "main",
                                "image": self.image.to_string(),
                                "args": ["runner"],
                            }
                        ]
                    }
                }
            }
        }))
        .expect("runner must parse")
    }

    pub fn service(&self, name: &str) -> core::Service {
        let labels = Some(("server".to_string(), name.to_string()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "namespace": format!("linkerd-k8s-test-{}", self.id),
                "name": format!("server-{}", name),
                "labels": labels,
            },
            "spec": {
                "ports": [
                    {
                        "name": "admin-http",
                        "port": 9080,
                        "protocol": "TCP",
                    },
                    {
                        "name": "test-http",
                        "port": 8080,
                        "protocol": "TCP",
                    }
                ],
                "selector": labels,
                "type": "ClusterIP",
            }
        }))
        .expect("server deployment must parse")
    }

    pub fn server(&self, name: &str, linkerd: Option<&LinkerdConfig>) -> apps::Deployment {
        let labels = Some(("server".to_string(), name.to_string()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let pod_annotations = linkerd
            .map(|c| c.to_annotations(&self.proxy_image))
            .unwrap_or_default();

        serde_json::from_value(serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {
                "namespace": format!("linkerd-k8s-test-{}", self.id),
                "name": format!("server-{}", name),
                "labels": labels,
            },
            "spec": {
                "selector": {
                    "matchLabels": labels,
                },
                "template": {
                    "metadata": {
                        "labels": labels,
                        "annotations": pod_annotations,
                    },
                    "spec": {
                        "containers": [
                            {
                                "name": "main",
                                "image": self.image.to_string(),
                                "args": ["server"],
                                "ports": [
                                    {"name": "admin-http", "containerPort": 9080},
                                    {"name": "test-http", "containerPort": 8080},
                                ],
                                "livenessProbe": {
                                    "httpGet": {
                                        "path": "/live",
                                        "port": "admin-http",
                                    }
                                },
                                "readinessProbe": {
                                    "httpGet": {
                                        "path": "/ready",
                                        "port": "admin-http",
                                    }
                                }
                            }
                        ]
                    }
                }
            }
        }))
        .expect("server deployment must parse")
    }
}

// === impl LinkerdConfig ===

impl Default for LinkerdConfig {
    fn default() -> Self {
        LinkerdConfig::Enabled {
            config: HashMap::default(),
        }
    }
}

impl LinkerdConfig {
    fn to_annotations(&self, image: &Image) -> BTreeMap<String, String> {
        let mut annotations = BTreeMap::<String, String>::default();

        let config = match self {
            LinkerdConfig::Enabled { ref config } => {
                annotations.insert("linkerd.io/inject".into(), "enabled".into());
                config
            }
            LinkerdConfig::Ingress { ref config } => {
                annotations.insert("linkerd.io/inject".into(), "ingress".into());
                config
            }
        };

        for (k, v) in config.iter() {
            annotations.insert(format!("config.linkerd.io/{}", k), v.into());
        }

        annotations.insert("config.linkerd.io/proxy-image".into(), image.repo.clone());
        if let Some(v) = image.tag.as_ref() {
            annotations.insert("config.linkerd.io/proxy-version".into(), v.into());
        }

        annotations
    }
}
