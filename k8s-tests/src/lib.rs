use anyhow::Result;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Describes a server interface exposed by a set of pods.
#[derive(Clone, Debug, CustomResource, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "proxy.l5d.io",
    version = "v1alpha1",
    kind = "Test",
    status = "TestStatus",
    namespaced
)]
//#[serde(rename_all = "camelCase")]
pub struct TestSpec {}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct TestStatus {}

pub async fn run(_client: kube::Client) -> Result<()> {
    todo!()
}
