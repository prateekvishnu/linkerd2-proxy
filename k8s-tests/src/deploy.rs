#![allow(dead_code)] // FIXME

use anyhow::Result;
use k8s_openapi::{
    api::{apps::v1 as apps, batch::v1 as batch, core::v1 as core},
    apimachinery::pkg::{apis::meta::v1::LabelSelector, util::intstr::IntOrString},
};
use kube::api::ObjectMeta;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use structopt::StructOpt;

#[derive(Clone, Debug, StructOpt)]
pub struct Args {
    #[structopt(long, default_value = "ghcr.io/linkerd/k8s-tests:latest")]
    image: String,

    #[structopt(long)]
    proxy_image: String,

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
        write!(
            f,
            "{}:{}",
            self.repo,
            self.tag.as_deref().unwrap_or("latest")
        )
    }
}

impl Args {
    pub async fn run(self, client: kube::Client) -> Result<()> {
        super::apply_crds(client, 10).await?;

        Ok(())
    }
}

impl TestNs {
    pub fn namespace(&self) -> core::Namespace {
        core::Namespace {
            metadata: ObjectMeta {
                name: Some(format!("linkerd-k8s-test-{}", self.id)),
                ..ObjectMeta::default()
            },
            ..core::Namespace::default()
        }
    }

    pub fn runner(&self, name: &str, linkerd: Option<&LinkerdConfig>) -> batch::Job {
        let labels = Some(("runner".to_string(), name.to_string()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let pod_annotations = linkerd
            .map(|c| c.to_annotations(&self.proxy_image))
            .unwrap_or_default();

        batch::Job {
            metadata: ObjectMeta {
                namespace: Some(format!("linkerd-k8s-test-{}", self.id)),
                name: Some(format!("runner-{}", name)),
                labels: labels.clone(),
                ..ObjectMeta::default()
            },
            spec: Some(batch::JobSpec {
                template: core::PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels,
                        annotations: pod_annotations,
                        ..ObjectMeta::default()
                    }),
                    spec: Some(core::PodSpec {
                        containers: vec![core::Container {
                            name: "main".to_string(),
                            image: Some(self.image.to_string()),
                            args: vec!["runner".to_string()],
                            ..core::Container::default()
                        }],
                        ..core::PodSpec::default()
                    }),
                },
                ..batch::JobSpec::default()
            }),
            ..batch::Job::default()
        }
    }

    pub fn server(&self, name: &str, linkerd: Option<&LinkerdConfig>) -> apps::Deployment {
        let labels = Some(("server".to_string(), name.to_string()))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let pod_annotations = linkerd
            .map(|c| c.to_annotations(&self.proxy_image))
            .unwrap_or_default();

        apps::Deployment {
            metadata: ObjectMeta {
                namespace: Some(format!("linkerd-k8s-test-{}", self.id)),
                name: Some(format!("server-{}", name)),
                labels: labels.clone(),
                ..ObjectMeta::default()
            },
            spec: Some(apps::DeploymentSpec {
                selector: LabelSelector {
                    match_expressions: vec![],
                    match_labels: labels.clone(),
                },
                template: core::PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels,
                        annotations: pod_annotations,
                        ..ObjectMeta::default()
                    }),
                    spec: Some(core::PodSpec {
                        containers: vec![core::Container {
                            name: "main".to_string(),
                            image: Some(self.image.to_string()),
                            args: vec!["server".to_string()],
                            ports: vec![
                                core::ContainerPort {
                                    name: Some("admin-http".into()),
                                    container_port: 9080,
                                    ..core::ContainerPort::default()
                                },
                                core::ContainerPort {
                                    name: Some("test-http".into()),
                                    container_port: 8080,
                                    ..core::ContainerPort::default()
                                },
                            ],
                            liveness_probe: Some(core::Probe {
                                http_get: Some(core::HTTPGetAction {
                                    path: Some("/live".into()),
                                    port: IntOrString::String("admin-http".into()),
                                    ..core::HTTPGetAction::default()
                                }),
                                ..core::Probe::default()
                            }),
                            readiness_probe: Some(core::Probe {
                                http_get: Some(core::HTTPGetAction {
                                    path: Some("/ready".into()),
                                    port: IntOrString::String("admin-http".into()),
                                    ..core::HTTPGetAction::default()
                                }),
                                ..core::Probe::default()
                            }),
                            ..core::Container::default()
                        }],
                        ..core::PodSpec::default()
                    }),
                },
                ..apps::DeploymentSpec::default()
            }),
            ..apps::Deployment::default()
        }
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
