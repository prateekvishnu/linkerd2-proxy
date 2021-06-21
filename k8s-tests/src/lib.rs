#![deny(warnings, rust_2018_idioms)]
#![forbid(unsafe_code)]
#![allow(clippy::inconsistent_struct_constructor)]

pub mod deploy;
pub mod runner;
pub mod server;

use anyhow::{anyhow, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::{CustomResource, CustomResourceExt, ResourceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Describes a server interface exposed by a set of pods.
#[derive(Clone, Debug, CustomResource, Deserialize, Serialize, JsonSchema)]
#[kube(
    apiextensions = "v1",
    group = "test.proxy.l5d.io",
    version = "v1alpha1",
    kind = "TestResult",
    namespaced
)]
#[serde(rename_all = "camelCase")]
struct TestResultSpec {
    complete: bool,
    tests: Vec<Test>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Test {
    name: String,

    success: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    failure_message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Deploy {
    pub id: String,
    pub image: String,
    pub proxy_image: String,
}

pub async fn create_crds(client: kube::Client, timeout_secs: u32) -> Result<()> {
    let api = kube::Api::all(client);
    let crd = create_crd::<TestResult>(&api, timeout_secs).await?;
    info!(crd = %crd.name(), "Created");

    Ok(())
}

pub async fn apply_crds(client: kube::Client, timeout_secs: u32) -> Result<()> {
    let crd = apply_crd::<TestResult>(client, timeout_secs).await?;
    info!(crd = %crd.name(), "Applied");

    Ok(())
}

pub async fn delete_crds(client: kube::Client) -> Result<()> {
    let api = kube::Api::<CustomResourceDefinition>::all(client.clone());

    let name = TestResult::crd().name();
    api.delete(&name, &Default::default()).await?;
    info!(crd = %name, "Deleted");

    Ok(())
}

async fn apply_crd<T: CustomResourceExt>(
    client: kube::Client,
    timeout_secs: u32,
) -> Result<CustomResourceDefinition> {
    use kube::api::{Patch, PatchParams};

    let api = kube::Api::<CustomResourceDefinition>::all(client);

    let params = PatchParams::apply("apply");
    let crd = T::crd();
    let name = crd.name();
    api.patch(&name, &params, &Patch::Apply(crd)).await?;

    crd_accepted(&api, &name, timeout_secs)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "{} was not accepted in within {} seconds",
                name,
                timeout_secs
            )
        })
}

async fn create_crd<T: CustomResourceExt>(
    api: &kube::Api<CustomResourceDefinition>,
    timeout_secs: u32,
) -> Result<CustomResourceDefinition> {
    let crd = api
        .create(&kube::api::PostParams::default(), &T::crd())
        .await?;
    crd_accepted(api, &crd.name(), timeout_secs)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "{} was not accepted within {} seconds",
                crd.name(),
                timeout_secs
            )
        })
}

async fn crd_accepted(
    api: &kube::Api<CustomResourceDefinition>,
    name: &str,
    timeout_secs: u32,
) -> Result<Option<CustomResourceDefinition>> {
    use futures::prelude::*;
    use kube::api::{ListParams, WatchEvent};

    let mut stream = {
        let lp = ListParams::default()
            .fields(&format!("metadata.name={}", name))
            .timeout(timeout_secs);
        api.watch(&lp, "0").await?.boxed_local()
    };

    while let Some(status) = stream.try_next().await? {
        match status {
            WatchEvent::Added(crd) if is_accepted(&crd) => {
                return Ok(Some(crd));
            }
            WatchEvent::Modified(crd) if is_accepted(&crd) => {
                return Ok(Some(crd));
            }
            _ => {}
        }
    }

    Ok(None)
}

fn is_accepted(crd: &CustomResourceDefinition) -> bool {
    crd.status
        .as_ref()
        .map(|s| {
            s.conditions
                .iter()
                .any(|c| c.type_ == "NamesAccepted" && c.status == "True")
        })
        .unwrap_or(false)
}
