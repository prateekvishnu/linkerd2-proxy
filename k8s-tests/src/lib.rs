#![deny(warnings, rust_2018_idioms)]
#![forbid(unsafe_code)]
#![allow(clippy::inconsistent_struct_constructor)]

pub mod deploy;
pub mod runner;
pub mod server;

use anyhow::{anyhow, Result};
use k8s_openapi::{
    api::apps::v1 as apps,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
};
use kube::{CustomResource, CustomResourceExt, ResourceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::info;

const FIELD_MANAGER: &str = "k8s-tests.proxy.linkerd.io";

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

pub async fn create_crds(client: kube::Client, timeout: Duration, dry_run: bool) -> Result<()> {
    let api = kube::Api::all(client);
    let crd = create_crd::<TestResult>(&api, timeout, dry_run).await?;
    info!(crd = %crd.name(), ?dry_run, "Created");

    Ok(())
}

pub async fn apply_crds(client: kube::Client, timeout: Duration, dry_run: bool) -> Result<()> {
    let crd = apply_crd::<TestResult>(client, timeout, dry_run).await?;
    info!(crd = %crd.name(), ?dry_run, "Applied");

    Ok(())
}

pub async fn delete_crds(client: kube::Client) -> Result<()> {
    let api = kube::Api::<CustomResourceDefinition>::all(client.clone());

    let name = TestResult::crd().name();
    api.delete(&name, &Default::default()).await?;
    info!(crd = %name, "Deleted");

    Ok(())
}

/// Updates a T-typed CRD in the cluster and wait for it to accepted by the API server. Fails if the
/// CRD already exists.
async fn apply_crd<T: CustomResourceExt>(
    client: kube::Client,
    timeout: Duration,
    dry_run: bool,
) -> Result<CustomResourceDefinition> {
    use kube::api::{Patch, PatchParams};

    let api = kube::Api::<CustomResourceDefinition>::all(client);

    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.to_string()),
        dry_run,
        force: false,
    };
    let crd = T::crd();
    let name = crd.name();
    let crd = api.patch(&name, &params, &Patch::Apply(crd)).await?;

    if dry_run {
        return Ok(crd);
    }

    crd_accepted(&api, &name, timeout)
        .await?
        .ok_or_else(|| anyhow!("{} was not accepted in within {:?}", name, timeout))
}

/// Creates a T-typed CRD in the cluster and wait for it to accepted by the API server. Fails if the
/// CRD already exists.
async fn create_crd<T: CustomResourceExt>(
    api: &kube::Api<CustomResourceDefinition>,
    timeout: Duration,
    dry_run: bool,
) -> Result<CustomResourceDefinition> {
    let params = kube::api::PostParams {
        dry_run,
        field_manager: Some(FIELD_MANAGER.to_string()),
    };
    let crd = api.create(&params, &T::crd()).await?;

    if dry_run {
        return Ok(crd);
    }

    crd_accepted(api, &crd.name(), timeout)
        .await?
        .ok_or_else(|| anyhow!("{} was not accepted within {:?}", crd.name(), timeout))
}

/// Waits for the named CRD to be accepted by the API server.
async fn crd_accepted(
    api: &kube::Api<CustomResourceDefinition>,
    name: &str,
    timeout: Duration,
) -> Result<Option<CustomResourceDefinition>> {
    use futures::prelude::*;
    use kube::api::{ListParams, WatchEvent};

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

    let mut stream = {
        let lp = ListParams::default()
            .fields(&format!("metadata.name={}", name))
            .timeout(timeout.as_secs() as u32);
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

async fn create_deploy(
    client: kube::Client,
    deploy: apps::Deployment,
    timeout: Duration,
    dry_run: bool,
) -> Result<apps::Deployment> {
    use kube::api::PostParams;

    let api = kube::Api::<apps::Deployment>::namespaced(client, &*deploy.namespace().unwrap());
    let deploy = {
        let params = PostParams {
            dry_run,
            field_manager: Some(FIELD_MANAGER.to_string()),
        };
        api.create(&params, &deploy).await?
    };

    if dry_run {
        return Ok(deploy);
    }

    let name = deploy.name();
    deploy_available(&api, &name, timeout)
        .await?
        .ok_or_else(|| anyhow!("{} was not available in within {:?}", name, timeout))
}

async fn deploy_available(
    api: &kube::Api<apps::Deployment>,
    name: &str,
    timeout: Duration,
) -> Result<Option<apps::Deployment>> {
    use futures::prelude::*;
    use kube::api::{ListParams, WatchEvent};

    fn is_available(d: &apps::Deployment) -> bool {
        if let Some(s) = d.status.as_ref() {
            return s.available_replicas.unwrap_or(0) > 0;
        }

        false
    }

    let mut stream = {
        let lp = ListParams::default()
            .fields(&format!("metadata.name={}", name))
            .timeout(timeout.as_secs() as u32);
        api.watch(&lp, "0").await?.boxed_local()
    };

    while let Some(status) = stream.try_next().await? {
        match status {
            WatchEvent::Added(d) if is_available(&d) => {
                return Ok(Some(d));
            }
            WatchEvent::Modified(d) if is_available(&d) => {
                return Ok(Some(d));
            }
            _ => {}
        }
    }

    Ok(None)
}
