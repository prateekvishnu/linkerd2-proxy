#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _trace = linkerd_tracing::test::trace_init();

    linkerd_k8s_tests::run().await
}
