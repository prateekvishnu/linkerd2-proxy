use anyhow::Result;
use structopt::StructOpt;

#[derive(Clone, Debug, StructOpt)]
pub struct Args {
    #[structopt(short, long, default_value = "default", env = "NAMESPACE")]
    namespace: String,
}

impl Args {
    pub async fn run(self, _client: kube::Client) -> Result<()> {
        todo!()
    }
}
