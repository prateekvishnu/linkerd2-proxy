#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _trace = linkerd_tracing::test::trace_init();

    let client = kube::Client::try_default().await?;
    linkerd_k8s_tests::run(client).await
}
